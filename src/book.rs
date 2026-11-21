use serde::{Deserialize, Serialize};
use std::{fs, io::{Read, Write}, net::IpAddr, path::Path};
use tempfile::NamedTempFile;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Desktop,
    Laptop,
    Server,
    Iot,
    #[default]
    #[serde(other)]
    Unspecified,
}
impl DeviceKind {
    pub const ALL: [Self; 5] = [Self::Unspecified, Self::Desktop, Self::Laptop, Self::Server, Self::Iot];
    pub fn label(self) -> &'static str {
        match self {
            Self::Unspecified => "Unspecified device",
            Self::Desktop => "Desktop",
            Self::Laptop => "Laptop",
            Self::Server => "Server / NAS",
            Self::Iot => "IoT device",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub alias: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "kind_unspecified")]
    pub kind: DeviceKind,
    pub host: String,
    pub lan: String,
    pub tailscale: String,
    pub user: String,
    pub port: u16,
    pub tags: String,
    pub notes: String,
}
fn kind_unspecified(kind: &DeviceKind) -> bool { *kind == DeviceKind::Unspecified }
impl Default for Device {
    fn default() -> Self {
        Self { alias: String::new(), name: String::new(), kind: DeviceKind::Unspecified, host: String::new(),
            lan: String::new(), tailscale: String::new(), user: String::new(), port: 22,
            tags: String::new(), notes: String::new() }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Route { #[default] Saved, Lan, Tailscale }
impl Route {
    pub const ALL: [Self; 3] = [Self::Saved, Self::Lan, Self::Tailscale];
    pub fn label(self) -> &'static str {
        match self { Self::Saved => "Saved host", Self::Lan => "LAN", Self::Tailscale => "Tailscale" }
    }
}
fn plain(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && !s.starts_with('-') &&
        s.bytes().all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
}
pub(crate) fn host_ok(s: &str) -> bool {
    s.parse::<IpAddr>().is_ok() || (!s.is_empty() && s.len() <= 253 &&
        s.trim_end_matches('.').split('.').all(|part| {
            !part.is_empty() && part.len() <= 63 && !part.starts_with('-') && !part.ends_with('-') &&
                part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        }))
}
impl Device {
    pub fn address(&self, route: Route) -> &str {
        match route {
            Route::Saved => if !self.host.is_empty() { &self.host } else if !self.tailscale.is_empty() { &self.tailscale } else { &self.lan },
            Route::Lan => &self.lan,
            Route::Tailscale => &self.tailscale,
        }
    }
    pub fn title(&self) -> &str {
        if self.name.is_empty() { &self.alias } else { &self.name }
    }
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        self.port.to_string().contains(&query) ||
            [&self.alias,&self.name,&self.host,&self.lan,&self.tailscale,&self.user,&self.tags]
                .iter().any(|s| s.to_lowercase().contains(&query))
    }
    pub fn validate(&self) -> Result<(), String> {
        if !plain(&self.alias) { return Err("Enter a short alias using letters, digits, ., _ or - (not a leading -).".into()); }
        if !plain(&self.user) { return Err("Enter a simple SSH username using letters, digits, ., _ or - (not a leading -).".into()); }
        if self.port == 0 { return Err("SSH port must be 1–65535.".into()); }
        if self.address(Route::Saved).is_empty() { return Err("Enter a saved host, LAN address or Tailscale name.".into()); }
        for h in [&self.host, &self.lan, &self.tailscale] {
            if !h.is_empty() && !host_ok(h) { return Err("Use a hostname or IP address, without a URL scheme, port or shell characters.".into()); }
        }
        if self.name.len() > 200 || self.tags.len() > 500 || self.notes.len() > 16_000 {
            return Err("Display name, tags or notes are too long.".into());
        }
        Ok(())
    }
    pub fn ssh(&self, route: Route) -> Result<String, String> {
        let args = self.ssh_args(route)?;
        Ok(format!("ssh {}", args.join(" ")))
    }
    pub fn ssh_args(&self, route: Route) -> Result<Vec<String>, String> {
        self.validate()?;
        let host = self.address(route);
        if host.is_empty() { return Err(format!("{} has no {} address.", self.alias, route.label())); }
        let mut a = vec!["-l".to_owned(), self.user.clone()];
        if self.port != 22 { a.extend(["-p".to_owned(), self.port.to_string()]); }
        a.extend(["--".to_owned(), host.into()]);
        Ok(a)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Book { pub version: u8, pub theme: String, #[serde(default)] pub copy_on_select: bool, pub devices: Vec<Device> }
impl Default for Book {
    fn default() -> Self { Self { version: 1, theme: "lab-navy".into(), copy_on_select: false, devices: Vec::new() } }
}
impl Book {
    pub fn remove_reviewed(&mut self, expected:&Device) -> Result<(usize,Device),String> {
        let i=self.devices.iter().position(|d|d==expected)
            .ok_or("This entry changed or was already removed. Review the current entry first.")?;
        Ok((i,self.devices.remove(i)))
    }
    pub fn restore_removed(&mut self, index:usize, device:Device)->Result<(),String>{
        device.validate()?;
        if self.devices.iter().any(|d|d.alias.eq_ignore_ascii_case(&device.alias)){
            return Err("This alias is already in use. Undo will not overwrite it.".into());
        }
        self.devices.insert(index.min(self.devices.len()),device);
        Ok(())
    }
    pub fn read(path: &Path) -> Result<Self, String> {
        let s = match fs::File::open(path) {
            Ok(source) => bounded(source, 8 * 1024 * 1024)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.to_string()),
        };
        let b: Self = serde_json::from_slice(&s).map_err(err)?;
        if b.version != 1 { return Err("This inventory was written by a different version of DrakOps.".into()); }
        if b.devices.len() > 5000 { return Err("Inventory exceeds the device limit.".into()); }
        for (i,d) in b.devices.iter().enumerate() {
            d.validate()?;
            if b.devices[..i].iter().any(|x| x.alias.eq_ignore_ascii_case(&d.alias)) {
                return Err("Duplicate alias in the inventory.".into());
            }
        }
        Ok(b)
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let dir = path.parent().ok_or("Inventory directory is missing.")?;
        fs::create_dir_all(dir).map_err(err)?;
        let bytes = serde_json::to_vec_pretty(self).map_err(err)?;
        if bytes.len() > 8 * 1024 * 1024 || self.devices.len() > 5000 { return Err("Inventory exceeds the size limit.".into()); }
        let mut f = NamedTempFile::new_in(dir).map_err(err)?;
        f.write_all(&bytes).map_err(err)?;
        f.as_file().sync_all().map_err(err)?;
        f.persist(path).map_err(err)?;
        Ok(())
    }
}
pub fn bounded<R: Read>(reader: R, limit: u64) -> Result<Vec<u8>,String> {
    let mut bytes = Vec::new();
    reader.take(limit+1).read_to_end(&mut bytes).map_err(err)?;
    if bytes.len() as u64 > limit { return Err("File exceeds the size limit.".into()); }
    Ok(bytes)
}
fn err(e: impl std::fmt::Display) -> String { e.to_string() }

const HEADER: [&str; 4] = ["alias", "hostname", "user", "port"];
pub fn parse_csv(bytes: &[u8]) -> Result<Vec<Device>, String> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let mut r = csv::ReaderBuilder::new().trim(csv::Trim::All).from_reader(bytes);
    if !r.headers().map_err(err)?.iter().eq(HEADER) {
        return Err("Expected this hosts.csv header: alias,hostname,user,port".into());
    }
    let mut rows: Vec<Device> = Vec::new();
    for (i, rec) in r.records().enumerate() {
        if i >= 5000 { return Err("CSV exceeds the 5000-device review limit.".into()); }
        let rec = rec.map_err(err)?;
        let d = Device { alias: rec[0].into(), host: rec[1].into(), user: rec[2].into(),
            port: rec[3].parse().map_err(|_| format!("Record {}: invalid port.",i+1))?,
            ..Device::default() };
        d.validate().map_err(|e| format!("Record {}: {e}",i+1))?;
        if rows.iter().any(|x| x.alias.eq_ignore_ascii_case(&d.alias)) {
            return Err(format!("Duplicate alias in CSV: {}", d.alias));
        }
        rows.push(d);
    }
    Ok(rows)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn csv_import_accepts_bom_crlf_and_keeps_endpoint() {
        let b=b"\xef\xbb\xbfalias,hostname,user,port\r\nfedora,framework-desktop,drak,22\r\n";
        let d=parse_csv(b).unwrap();
        assert_eq!((&*d[0].alias,&*d[0].host,&*d[0].user,d[0].port),("fedora","framework-desktop","drak",22));
    }
    #[test] fn reject_unsafe_fields() {
        for bytes in [
            "alias,hostname,user,port\n-x,foo,me,22",
            "alias,hostname,user,port\nok,foo;echo,me,22",
            "alias,hostname,user,port\nok,foo,me&who,22",
            "alias,hostname,user,port\nok,foo,me,0",
            "alias,hostname,user,port\nok,foo,me,22\nOK,bar,me,22",
            "alias,hostname,user,port\nok,https://foo,me,22",
            "hostname,alias,user,port\nfoo,ok,me,22",
        ] { assert!(parse_csv(bytes.as_bytes()).is_err(),"{bytes}"); }
    }
    #[test] fn copies_and_search() {
        let d=Device{alias:"pve".into(),name:"Lab Mini".into(),user:"me".into(),lan:"2001:db8::1".into(),port:2202,..Device::default()};
        assert!(d.matches("mini") && d.matches("DB8"));
        assert_eq!(d.ssh(Route::Lan).unwrap(),"ssh -l me -p 2202 -- 2001:db8::1");
        assert!(d.ssh(Route::Tailscale).is_err());
    }
    #[test] fn inventory_persistence() {
        let dir=tempfile::tempdir().unwrap();
        let p=dir.path().join("book.json"); let mut b=Book::default();
        b.save(&p).unwrap(); b.theme="dlms-light".into(); b.save(&p).unwrap();
        assert_eq!(Book::read(&p).unwrap().theme,b.theme);
        let legacy:Book=serde_json::from_str(r#"{"version":1,"theme":"lab-navy","devices":[]}"#).unwrap();
        assert!(!legacy.copy_on_select);
        b.copy_on_select=true;b.save(&p).unwrap();assert!(Book::read(&p).unwrap().copy_on_select);

    }
}
