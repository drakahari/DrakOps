use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, io::{Read, Write}, net::IpAddr, path::Path};
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
/// Exact file bytes (or absence) observed by this instance, until its next successful save.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Revision(Option<Vec<u8>>);

pub(crate) const MAX_DEVICES: usize = 5000;

#[derive(Debug)]
pub enum SaveError {
    Busy,
    Conflict,
    Storage(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("Another DrakOps window is saving. Nothing was overwritten. Try again in a moment."),
            Self::Conflict => f.write_str("The saved inventory changed in another window. Nothing was overwritten. Reload the saved inventory before retrying."),
            Self::Storage(message) => f.write_str(message),
        }
    }
}

fn nonblocking_option(options: &mut fs::OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    let _ = options;
}

fn require_regular(metadata: fs::Metadata) -> std::io::Result<()> {
    if metadata.is_file() { Ok(()) }
    else { Err(std::io::Error::new(std::io::ErrorKind::InvalidInput,"Choose a regular file.")) }
}

pub(crate) fn open_regular(path: &Path) -> std::io::Result<fs::File> {
    require_regular(fs::metadata(path)?)?;
    let mut options=fs::OpenOptions::new();
    options.read(true);
    // Reject a file that became a pipe before open without letting the open wait.
    nonblocking_option(&mut options);
    let file=options.open(path)?;
    require_regular(file.metadata()?)?;
    Ok(file)
}

struct InventoryLock(fs::File);

impl InventoryLock {
    fn acquire(file: fs::File) -> Result<Self, SaveError> {
        match fs::File::try_lock(&file) {
            Ok(()) => Ok(Self(file)),
            Err(fs::TryLockError::WouldBlock) => Err(SaveError::Busy),
            Err(fs::TryLockError::Error(e)) => Err(SaveError::Storage(err(e))),
        }
    }
}

impl Drop for InventoryLock {
    fn drop(&mut self) {
        // A concurrently forked child may briefly inherit the same open file
        // description before exec. Explicitly unlock rather than waiting for
        // its last descriptor to close. The File still closes on every path.
        // An unlock error must not turn an already-persisted save into a failure.
        let _ = fs::File::unlock(&self.0);
    }
}

fn inventory_bytes(path: &Path) -> Result<Revision, String> {
    match open_regular(path) {
        Ok(file) => bounded(file, 8 * 1024 * 1024).map(|bytes| Revision(Some(bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Revision(None)),
        Err(e) => Err(err(e)),
    }
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
    pub fn read(path: &Path) -> Result<(Self, Revision), String> {
        let revision = inventory_bytes(path)?;
        let Some(bytes) = &revision.0 else { return Ok((Self::default(), revision)); };
        let b: Self = serde_json::from_slice(bytes).map_err(err)?;
        if b.version != 1 { return Err("This inventory was written by a different version of DrakOps.".into()); }
        if b.devices.len() > MAX_DEVICES { return Err("Inventory exceeds the device limit.".into()); }
        let mut aliases=HashSet::with_capacity(b.devices.len());
        for d in &b.devices {
            d.validate()?;
            if !aliases.insert(d.alias.to_ascii_lowercase()) {
                return Err("Duplicate alias in the inventory.".into());
            }
        }
        Ok((b, revision))
    }
    /// All writers must lock the stable sidecar, not the inventory inode which is replaced.
    /// Cooperating processes cannot check the same old revision and both commit a write.
    pub fn save(&self, path: &Path, expected: &Revision) -> Result<Revision, SaveError> {
        let storage = |e: std::io::Error| SaveError::Storage(err(e));
        let dir = path.parent().ok_or_else(|| SaveError::Storage("Inventory directory is missing.".into()))?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| SaveError::Storage(err(e)))?;
        if bytes.len() > 8 * 1024 * 1024 || self.devices.len() > MAX_DEVICES {
            return Err(SaveError::Storage("Inventory exceeds the size limit.".into()));
        }
        fs::create_dir_all(dir).map_err(storage)?;
        // Keep the sidecar on disk for the lifetime of the inventory; never unlink it at unlock.
        let lock_path = path.with_extension("lock");
        match fs::metadata(&lock_path) {
            Ok(metadata) => require_regular(metadata).map_err(storage)?,
            Err(e) if e.kind()==std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(storage(e)),
        }
        let mut options=fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        nonblocking_option(&mut options);
        let lock=options.open(lock_path).map_err(storage)?;
        require_regular(lock.metadata().map_err(storage)?).map_err(storage)?;
        let _lock=InventoryLock::acquire(lock)?;
        if inventory_bytes(path).map_err(SaveError::Storage)? != *expected {
            return Err(SaveError::Conflict);
        }
        let mut f = NamedTempFile::new_in(dir).map_err(storage)?;
        f.write_all(&bytes).map_err(storage)?;
        f.as_file().sync_all().map_err(storage)?;
        f.persist(path).map_err(|e| storage(e.error))?;
        Ok(Revision(Some(bytes)))
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
    let mut aliases=HashSet::new();
    for (i, rec) in r.records().enumerate() {
        if i >= MAX_DEVICES { return Err("CSV exceeds the 5000-device review limit.".into()); }
        let rec = rec.map_err(err)?;
        let d = Device { alias: rec[0].into(), host: rec[1].into(), user: rec[2].into(),
            port: rec[3].parse().map_err(|_| format!("Record {}: invalid port.",i+1))?,
            ..Device::default() };
        d.validate().map_err(|e| format!("Record {}: {e}",i+1))?;
        if !aliases.insert(d.alias.to_ascii_lowercase()) {
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
        let p=dir.path().join("book.json");
        let (mut b, revision)=Book::read(&p).unwrap();
        let revision=b.save(&p,&revision).unwrap();
        b.theme="dlms-light".into();
        let revision=b.save(&p,&revision).unwrap();
        assert_eq!(Book::read(&p).unwrap().0.theme,b.theme);
        let legacy:Book=serde_json::from_str(r#"{"version":1,"theme":"lab-navy","devices":[]}"#).unwrap();
        assert!(!legacy.copy_on_select);
        b.copy_on_select=true;
        b.save(&p,&revision).unwrap();
        assert!(Book::read(&p).unwrap().0.copy_on_select);
    }

    #[test] fn concurrent_independent_inventory_saves_never_lose_winner() {
        use std::sync::Barrier;
        let dir=tempfile::tempdir().unwrap();
        let p=dir.path().join("book.json");
        let rendezvous=Barrier::new(2);
        let write = |theme: &str| {
            let (mut book,revision)=Book::read(&p).unwrap();
            book.theme=theme.into();
            rendezvous.wait();
            let result=book.save(&p,&revision);
            (book, revision, result)
        };
        let (left,right)=std::thread::scope(|scope| {
            let left=scope.spawn(||write("left"));
            let right=scope.spawn(||write("right"));
            (left.join().unwrap(),right.join().unwrap())
        });
        assert_ne!(left.2.is_ok(), right.2.is_ok());
        let (winner, loser)=if left.2.is_ok(){(left,right)}else{(right,left)};
        assert!(matches!(loser.2, Err(SaveError::Busy | SaveError::Conflict)));
        assert!(matches!(loser.0.save(&p,&loser.1), Err(SaveError::Conflict)));
        assert_eq!(Book::read(&p).unwrap().0.theme,winner.0.theme);
    }

    #[cfg(unix)]
    #[test]
    fn dropping_guard_unlocks_even_while_a_duplicate_descriptor_is_open() {
        let dir=tempfile::tempdir().unwrap();
        let path=dir.path().join("book.lock");
        let open=||fs::OpenOptions::new().read(true).write(true).create(true).truncate(false)
            .open(&path).unwrap();
        let original=open();
        let duplicate=original.try_clone().unwrap();
        let held=InventoryLock::acquire(original).unwrap();
        assert!(matches!(InventoryLock::acquire(open()),Err(SaveError::Busy)));
        drop(held);
        let next=InventoryLock::acquire(open()).expect("guard must unlock while the duplicate remains open");
        drop(next);
        drop(duplicate);
    }

    #[test]
    #[ignore = "helper invoked by sidecar_lock_prevents_other_process_from_saving"]
    fn sidecar_lock_child() {
        let Some(path)=std::env::var_os("DRAKOPS_TEST_SIDECAR") else {return;};
        let lock=fs::OpenOptions::new().read(true).write(true).create_new(true).open(path).unwrap();
        lock.try_lock().unwrap();
        println!("READY");
        std::io::stdout().flush().unwrap();
        let mut done=[0];
        std::io::stdin().read_exact(&mut done).unwrap();
    }

    #[test] fn sidecar_lock_prevents_other_process_from_saving() {
        use std::{io::{BufRead, BufReader},process::{Command, Stdio}};
        let dir=tempfile::tempdir().unwrap();
        let path=dir.path().join("book.json");
        let (book,revision)=Book::read(&path).unwrap();
        let mut child=Command::new(std::env::current_exe().unwrap())
            .args(["--ignored","--exact","book::tests::sidecar_lock_child","--nocapture"])
            .env("DRAKOPS_TEST_SIDECAR",path.with_extension("lock"))
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut ready=false;
        let mut stdout=BufReader::new(child.stdout.take().unwrap());
        for line in (&mut stdout).lines() {
            // libtest may print the helper's name in front of the readiness marker.
            if line.unwrap().trim_end().ends_with("READY"){ready=true;break;}
        }
        let result=if ready { Some(book.save(&path,&revision)) } else { None };
        let signal=child.stdin.take().unwrap().write_all(b"x");
        let mut rest=String::new();
        stdout.read_to_string(&mut rest).unwrap();
        let status=child.wait().unwrap();
        assert!(ready && signal.is_ok() && status.success());
        assert!(matches!(result,Some(Err(SaveError::Busy))));
        assert!(!path.exists());
        book.save(&path,&revision).unwrap();
    }

    #[test] fn external_change_is_never_overwritten_and_reload_can_save() {
        let dir=tempfile::tempdir().unwrap();
        let p=dir.path().join("book.json");
        let (_,missing)=Book::read(&p).unwrap();
        fs::write(&p,b"cannot parse").unwrap();
        assert!(matches!(Book::default().save(&p,&missing), Err(SaveError::Conflict)));
        assert_eq!(fs::read(&p).unwrap(),b"cannot parse");
        fs::remove_file(&p).unwrap();
        Book::default().save(&p,&missing).unwrap();
        let (mut book, fresh)=Book::read(&p).unwrap();
        book.theme="dlms-light".into();
        book.save(&p,&fresh).unwrap();
        assert_eq!(Book::read(&p).unwrap().0.theme,"dlms-light");
    }
}
