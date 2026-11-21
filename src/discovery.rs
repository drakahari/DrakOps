//! Import suggestions from the local Tailscale client. No probing or authentication.
use crate::book::{Device, Route};
use serde::Deserialize;
use std::{collections::BTreeMap, io::Read, net::IpAddr, path::PathBuf, process::{Command, Stdio}, thread, time::{Duration, Instant}};

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Status {
    backend_state: String,
    peer: Option<BTreeMap<String, Peer>>,
    #[serde(rename = "Self")]
    local: Option<Peer>,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Peer {
    #[serde(rename = "ID")]
    id: String,
    host_name: String,
    #[serde(rename = "DNSName")]
    dns_name: String,
    #[serde(rename = "TailscaleIPs")]
    tailscale_ips: Vec<IpAddr>,
    online: bool,
}
#[derive(Clone)]
pub struct Suggestion {
    pub address: String,
    pub name: String,
    pub alias: String,
    pub online: bool,
    ips: Vec<IpAddr>,
}
pub fn parse(bytes: &[u8]) -> Result<Vec<Suggestion>, String> {
    let status: Status = serde_json::from_slice(bytes).map_err(|_| "The local Tailscale client returned unreadable status.")?;
    if status.backend_state != "Running" { return Err("Tailscale is not connected. Sign in using the Tailscale app and try again.".into()); }
    let own = status.local.unwrap_or_default();
    let mut rows = Vec::<Suggestion>::new();
    for p in status.peer.unwrap_or_default().into_values() {
        if (p.id.is_empty() || p.id == own.id) || p.tailscale_ips.is_empty() || p.tailscale_ips.iter().any(|ip|own.tailscale_ips.contains(ip)) { continue; }
        let dns = p.dns_name.trim_end_matches('.');
        let address = if !dns.is_empty() && crate::book::host_ok(dns) { dns.to_owned() }
            else { p.tailscale_ips.iter().find(|x|x.is_ipv4()).unwrap_or(&p.tailscale_ips[0]).to_string() };
        if rows.iter().any(|r| r.address.eq_ignore_ascii_case(&address) || r.ips.iter().any(|ip|p.tailscale_ips.contains(ip))) { continue; }
        let source = if dns.is_empty() {p.host_name.as_str()} else {dns.split('.').next().unwrap_or_default()};
        let mut alias: String = source.to_ascii_lowercase().chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' {c} else {'-'})
            .take(55).collect();
        alias = alias.trim_matches('-').into();
        if alias.is_empty() { alias = "peer".into(); }
        let name: String = p.host_name.chars().filter(|c| !c.is_control() && !matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')).take(48).collect();
        rows.push(Suggestion{address, name: if name.is_empty() {alias.clone()} else {name}, alias, online:p.online,ips:p.tailscale_ips});
        if rows.len()==2000 {break;}
    }
    rows.sort_by_key(|r| (r.name.to_ascii_lowercase(),r.address.to_ascii_lowercase()));
    Ok(rows)
}
pub fn existing<'a>(devices: &'a [Device], suggestion: &Suggestion) -> Option<&'a Device> {
    devices.iter().find(|d| Route::ALL.iter().any(|r| { let a=d.address(*r).trim_end_matches('.'); a.eq_ignore_ascii_case(&suggestion.address) || a.parse::<IpAddr>().is_ok_and(|ip|suggestion.ips.contains(&ip)) }))
}
fn cli() -> Option<PathBuf> {
    let mut paths = Vec::new();
    #[cfg(target_os="windows")]
    if let Some(root) = std::env::var_os("ProgramFiles") {
        paths.push(PathBuf::from(root).join("Tailscale").join("tailscale.exe"));
    }
    #[cfg(target_os="macos")]
    paths.extend(["/opt/homebrew/bin/tailscale","/usr/local/bin/tailscale","/Applications/Tailscale.app/Contents/MacOS/Tailscale"].map(PathBuf::from));
    #[cfg(target_os="linux")]
    paths.extend(["/usr/bin/tailscale","/usr/local/bin/tailscale","/usr/sbin/tailscale"].map(PathBuf::from));
    paths.into_iter().find(|p|p.is_file())
}
pub fn discover() -> Result<Vec<Suggestion>, String> {
    let path = cli().ok_or("Tailscale client not found. You can still add a device manually or import hosts.csv.")?;
    let mut cmd = Command::new(path);
    cmd.args(["status","--json"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(target_os="macos")]
    cmd.env("TAILSCALE_BE_CLI","1");
    #[cfg(target_os="windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let mut process = cmd.spawn().map_err(|_|"Could not start the local Tailscale client.")?;
    let stream = process.stdout.take().ok_or("Tailscale did not produce status output.")?;
    let read = thread::spawn(move || {
        let mut out = Vec::new();
        stream.take(8 * 1024 * 1024 + 1).read_to_end(&mut out).map(|_|out)
    });
    let expires = Instant::now() + Duration::from_secs(7);
    loop {
        match process.try_wait() {
            Ok(Some(result)) => {
                if !result.success() {return Err("Could not read local Tailscale status. Make sure Tailscale is connected and this Windows/macOS/Linux user can run its CLI. You can add a device manually instead.".into());}
                let bytes = read.join().map_err(|_|"Could not read Tailscale output.")?.map_err(|_|"Could not read Tailscale output.")?;
                if bytes.len() > 8 * 1024 * 1024 {return Err("Tailscale status exceeded the review limit.".into());}
                return parse(&bytes);
            }
            Ok(None) if Instant::now() < expires => thread::sleep(Duration::from_millis(60)),
            _ => {
                let _ = process.kill();
                let _ = process.wait();
                return Err("The local Tailscale client did not respond in time. Try again or add the device manually.".into());
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_contains_only_peers_and_inert_names() {
        let bytes=br#"{"BackendState":"Running","Self":{"ID":"self","TailscaleIPs":["100.64.0.1"]},"Peer":{
          "a":{"ID":"self","DNSName":"self.example.","TailscaleIPs":["100.64.0.1"]},
          "b":{"ID":"pve","HostName":"PVE","DNSName":"pve.tail.example.","TailscaleIPs":["100.64.0.2"],"Online":true},
          "c":{"ID":"unsafe","HostName":"bad; touch","DNSName":"-bad;touch","TailscaleIPs":["100.64.0.3"],"Online":false},
          "d":{"ID":"missing-ip","HostName":"No address"},
          "e":{"ID":"dup","DNSName":"pve.tail.example.","TailscaleIPs":["100.64.0.4"]}
       }}"#;
       let peers=parse(bytes).unwrap();
       assert_eq!(peers.len(),2);
       let p=peers.iter().find(|p|p.online).unwrap();
       assert_eq!(p.address,"pve.tail.example");
       assert_eq!(p.alias,"pve");
       assert_eq!(peers.iter().find(|p|!p.online).unwrap().address,"100.64.0.3");
       let b=[Device{tailscale:"PVE.TAIL.EXAMPLE.".into(),..Device::default()}];
       assert!(existing(&b,p).is_some());
       assert!(existing(&[Device{host:"100.64.0.2".into(),..Device::default()}],p).is_some());
       assert_eq!(parse(br#"{"BackendState":"Running","Peer":null}"#).unwrap().len(),0);
       assert!(parse(br#"{"BackendState":"NeedsLogin"}"#).is_err());
    }
}
