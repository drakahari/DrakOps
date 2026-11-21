//! Bounded, user-triggered local DNS-SD browse. Never probes SSH.
use if_addrs::IfAddr;
use mdns_sd::{IfKind, RecvTimeoutError, ResolvedService, ServiceDaemon, ServiceEvent};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub const SERVICE: &str = "_ssh._tcp.local.";
const TIMEOUT: Duration = Duration::from_secs(4);
const CANCEL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_RECORDS: usize = 128;
const MAX_EVENTS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertisedHost {
    /// An unverified label from the network, never use as a command or trusted identity.
    pub name: String,
    /// Canonical advertised .local host. No claim of reachability or ownership.
    pub hostname: String,
    pub addresses: BTreeSet<IpAddr>,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Completed(Vec<AdvertisedHost>),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LanInterface {
    pub name: String,
    /// Helps users distinguish interfaces with unclear platform-specific names.
    pub addresses: BTreeSet<IpAddr>,
}

fn usable_interfaces() -> std::io::Result<Vec<if_addrs::Interface>> {
    Ok(if_addrs::get_if_addrs()?
        .into_iter()
        .filter(|i| i.is_oper_up() && !i.is_loopback() && !i.is_p2p() && !i.ip().is_unspecified())
        .collect())
}

/// Passive enumeration of this device's adapters. Ask for the LAN adapter
/// explicitly; do not assume that an adapter is physical from its name.
pub fn lan_interfaces() -> std::io::Result<Vec<LanInterface>> {
    let mut groups: BTreeMap<String, BTreeSet<IpAddr>> = BTreeMap::new();
    for i in usable_interfaces()? {
        groups.entry(i.name.clone()).or_default().insert(i.ip());
    }
    Ok(groups
        .into_iter()
        .map(|(name, addresses)| LanInterface { name, addresses })
        .collect())
}

fn same_subnet(network: &IfAddr, addr: IpAddr) -> bool {
    match (network, addr) {
        (IfAddr::V4(n), IpAddr::V4(a)) if n.prefixlen > 0 => {
            let mask = u32::from(n.netmask);
            let address = u32::from(a);
            let subnet = u32::from(n.ip) & mask;
            mask != 0
                && (address & mask) == subnet
                && (n.prefixlen >= 31 || (address != subnet && address != (subnet | !mask)))
        }
        (IfAddr::V6(n), IpAddr::V6(a)) if n.prefixlen > 0 => {
            let mask = u128::from(n.netmask);
            mask != 0 && (u128::from(a) & mask) == (u128::from(n.ip) & mask)
        }
        _ => false,
    }
}

fn normalize_hostname(s: &str) -> Option<String> {
    if !s.is_ascii() || s.len() > 253 {
        return None;
    }
    let host = s.strip_suffix('.').unwrap_or(s).to_ascii_lowercase();
    let prefix = host.strip_suffix(".local")?;
    if !prefix.split('.').all(|part| {
        !part.is_empty()
            && part.len() <= 63
            && !part.starts_with('-')
            && !part.ends_with('-')
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    }) {
        return None;
    }
    Some(host)
}

fn normalized(info: &ResolvedService, nets: &[IfAddr]) -> Option<(String, AdvertisedHost)> {
    if !info.ty_domain.eq_ignore_ascii_case(SERVICE)
        || info.port == 0
        || info.fullname.len() > 255
        || info.fullname.chars().any(char::is_control)
    {
        return None;
    }
    let ending = format!(".{SERVICE}");
    let key = info.fullname.to_ascii_lowercase();
    let prefix = key.strip_suffix(&ending)?;
    if prefix.is_empty() {
        return None;
    }
    let raw_name = &info.fullname[..prefix.len()];
    let hostname = normalize_hostname(&info.host)?;
    let checked: BTreeSet<_> = info
        .addresses
        .iter()
        .map(|a| a.to_ip_addr())
        .filter(|a| {
            if a.is_unspecified() || a.is_loopback() || a.is_multicast() {
                return false;
            }
            match a {
                IpAddr::V4(x) if x.is_broadcast() => return false,
                // Scoped IPv6 addresses aren't accepted by the current HomeOps address model.
                IpAddr::V6(x) if x.is_unicast_link_local() || x.to_ipv4_mapped().is_some() => {
                    return false;
                }
                _ => (),
            }
            nets.iter().any(|n| same_subnet(n, *a))
        })
        .collect();
    let addresses: BTreeSet<_> = checked.into_iter().take(32).collect();
    if addresses.is_empty() {
        return None;
    }
    // Omit bidi and other invisible controls that can spoof labels.
    let name: String = raw_name
        .chars()
        .filter(|c| {
            !matches!(c,
                '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' |
                '\u{2060}'..='\u{206f}' | '\u{feff}'
            )
        })
        .take(96)
        .collect();
    let name = if name.trim().is_empty() {
        hostname.clone()
    } else {
        name
    };
    Some((
        key,
        AdvertisedHost {
            name,
            hostname,
            addresses,
            port: info.port,
        },
    ))
}

#[derive(Default)]
struct Catalog {
    records: BTreeMap<String, AdvertisedHost>,
}

impl Catalog {
    fn event(&mut self, event: ServiceEvent, nets: &[IfAddr]) {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                if let Some((key, item)) = normalized(&info, nets)
                    && (self.records.len() < MAX_RECORDS || self.records.contains_key(&key))
                {
                    self.records.insert(key, item);
                }
            }
            ServiceEvent::ServiceRemoved(ty, name) if ty.eq_ignore_ascii_case(SERVICE) => {
                self.records.remove(&name.to_ascii_lowercase());
            }
            _ => (),
        }
    }

    fn finish(self) -> Vec<AdvertisedHost> {
        let mut endpoints: BTreeMap<(String, u16), AdvertisedHost> = BTreeMap::new();
        for record in self.records.into_values() {
            let key = (record.hostname.clone(), record.port);
            match endpoints.entry(key) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let merged = entry.get_mut();
                    if (record.name.chars().count(), &record.name)
                        < (merged.name.chars().count(), &merged.name)
                    {
                        merged.name = record.name;
                    }
                    merged.addresses.extend(record.addresses);
                    merged.addresses = std::mem::take(&mut merged.addresses)
                        .into_iter()
                        .take(32)
                        .collect();
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(record);
                }
            }
        }
        endpoints.into_values().collect()
    }
}

struct Daemon(ServiceDaemon);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.stop_browse(SERVICE);
        if let Ok(ack) = self.0.shutdown() {
            let _ = ack.recv_timeout(Duration::from_millis(200));
        }
    }
}

/// Call off the UI thread, only after the user chooses an interface and starts searching.
/// Integrator should treat results as suggestions, require username and explicit per-item
/// review, and avoid reusing the normal app's saved inventory or connection credentials.
pub fn browse(interface: &str, cancelled: &AtomicBool) -> Result<Outcome, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(Outcome::Cancelled);
    }
    let nets: Vec<_> = usable_interfaces()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|i| i.name == interface)
        .map(|i| i.addr)
        .collect();
    if nets.is_empty() {
        return Err("The selected local network interface is no longer available.".into());
    }

    let daemon = Daemon(ServiceDaemon::new().map_err(|e| e.to_string())?);
    daemon
        .0
        .disable_interface(IfKind::All)
        .map_err(|e| e.to_string())?;
    daemon
        .0
        .include_apple_p2p(false)
        .map_err(|e| e.to_string())?;
    daemon
        .0
        .accept_unsolicited(false)
        .map_err(|e| e.to_string())?;
    daemon
        .0
        .enable_interface(IfKind::Name(interface.into()))
        .map_err(|e| e.to_string())?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(Outcome::Cancelled);
    }
    let rx = daemon.0.browse(SERVICE).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + TIMEOUT;
    let mut catalog = Catalog::default();
    for _ in 0..MAX_EVENTS {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(Outcome::Cancelled);
        }
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(CANCEL_INTERVAL);
        if wait.is_zero() {
            break;
        }
        match rx.recv_timeout(wait) {
            Ok(event) => catalog.event(event, &nets),
            Err(RecvTimeoutError::Timeout) => (),
            Err(RecvTimeoutError::Disconnected) => {
                return Err("Local discovery stopped before the search finished.".into());
            }
        }
    }
    if cancelled.load(Ordering::Relaxed) {
        return Ok(Outcome::Cancelled);
    }
    Ok(Outcome::Completed(catalog.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use if_addrs::Ifv4Addr;
    use mdns_sd::ServiceInfo;

    fn net() -> IfAddr {
        IfAddr::V4(Ifv4Addr {
            ip: "192.0.2.10".parse().unwrap(),
            netmask: "255.255.255.0".parse().unwrap(),
            broadcast: Some("192.0.2.255".parse().unwrap()),
            prefixlen: 24,
        })
    }

    fn record(name: &str, host: &str, address: &str, port: u16) -> ResolvedService {
        ServiceInfo::new(
            SERVICE,
            name,
            host,
            address,
            port,
            [("user", "ignore-me")].as_slice(),
        )
        .unwrap()
        .as_resolved_service()
    }

    #[test]
    fn merges_duplicate_host_port_and_never_uses_txt_username() {
        let mut c = Catalog::default();
        c.event(
            ServiceEvent::ServiceResolved(Box::new(record(
                "Desk",
                "BOX.local.",
                "192.0.2.12",
                2222,
            ))),
            &[net()],
        );
        c.event(
            ServiceEvent::ServiceResolved(Box::new(record(
                "Desk duplicate",
                "box.local.",
                "192.0.2.13",
                2222,
            ))),
            &[net()],
        );
        let result = c.finish();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Desk");
        assert_eq!(result[0].hostname, "box.local");
        assert_eq!(result[0].port, 2222);
        assert_eq!(result[0].addresses.len(), 2);
    }

    #[test]
    fn rejects_wrong_service_bad_hosts_and_off_link_targets() {
        let mut r = record("Desk", "box.local.", "192.0.2.12", 22);
        r.ty_domain = "_http._tcp.local.".into();
        assert!(normalized(&r, &[net()]).is_none());
        r.ty_domain = SERVICE.into();
        r.port = 0;
        assert!(normalized(&r, &[net()]).is_none());
        assert!(normalize_hostname("not-the-lan.example.").is_none());
        assert!(normalize_hostname("--bad.local.").is_none());
        assert!(
            normalized(
                &record("Off-link", "not-the-lan.local.", "198.51.100.2", 22),
                &[net()]
            )
            .is_none()
        );
        assert!(
            normalized(
                &record("IPv6 scope needed", "lan.local.", "fe80::1", 22),
                &[net()]
            )
            .is_none()
        );
    }

    #[test]
    fn updates_and_removes_instance_without_duplicates() {
        let mut c = Catalog::default();
        let old = record("Desk", "box.local.", "192.0.2.12", 22);
        let new = record("Desk", "box.local.", "192.0.2.34", 22);
        let name = new.fullname.to_ascii_uppercase();
        c.event(ServiceEvent::ServiceResolved(Box::new(old)), &[net()]);
        c.event(ServiceEvent::ServiceResolved(Box::new(new)), &[net()]);
        assert_eq!(c.records.len(), 1);
        assert_eq!(
            c.records.values().next().unwrap().addresses,
            BTreeSet::from(["192.0.2.34".parse().unwrap()])
        );
        c.event(ServiceEvent::ServiceRemoved(SERVICE.into(), name), &[net()]);
        assert!(c.finish().is_empty());
    }

    #[test]
    fn cancellation_before_start_never_enumerates_or_binds() {
        assert_eq!(
            browse("never-a-real-adapter", &AtomicBool::new(true)).unwrap(),
            Outcome::Cancelled
        );
    }
}
