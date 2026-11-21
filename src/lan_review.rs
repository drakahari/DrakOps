use super::{App, Book, Device, egui, ComboBox, ScrollArea, TextEdit};
use crate::lan_discovery::{self, AdvertisedHost, LanInterface, Outcome};
use std::{collections::BTreeSet, net::IpAddr, sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc}, time::Duration};

type Worker = (mpsc::Receiver<Result<Outcome, String>>, Arc<AtomicBool>);
#[derive(Default)]
pub(super) struct LanState {
    pub(super) open: bool,
    adapters: Vec<LanInterface>,
    adapter: Option<String>,
    rows: Vec<Row>,
    worker: Option<Worker>,
    hint: String,
}
impl Drop for LanState {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl LanState {
    fn cancel(&self) {
        if let Some((_, stop)) = &self.worker { stop.store(true, Ordering::Relaxed); }
    }
    pub(super) fn show(&mut self, demo: bool) {
        if self.open { return; }
        self.open = true;
        if demo {
            self.rows = vec![Row::new(AdvertisedHost {
                name: "Sample LAN NAS".into(),
                hostname: "sample-nas.local".into(),
                addresses: BTreeSet::from([IpAddr::from([192, 0, 2, 34]), IpAddr::from([192, 0, 2, 35])]),
                port: 2222,
            })];
            self.hint = "Demo uses a sample advertisement without accessing your network.".into();
            return;
        }
        self.rows.clear();
        self.adapter = None;
        self.hint.clear();
        match lan_discovery::lan_interfaces() {
            Ok(adapters) => {
                self.adapters = adapters;
                if self.adapters.is_empty() { self.hint = "No available local adapters. Add the device manually.".into(); }
            }
            Err(_) => {
                self.adapters.clear();
                self.hint = "Could not list local adapters. Add the device manually.".into();
            }
        }
    }
}
struct Row {
    ad: AdvertisedHost,
    address: IpAddr,
    alias: String,
    user: String,
    port: String,
    selected: bool,
}
impl Row {
    fn new(ad: AdvertisedHost) -> Self {
        let address = *ad.addresses.first().expect("validated LAN advertisement contains an address");
        let alias = ad.hostname.split('.').next().unwrap_or("").trim_start_matches('-').to_owned();
        let port = ad.port.to_string();
        Self { ad, address, alias, user: String::new(), port, selected: false }
    }
}

fn apply(book: &Book, rows: &[Row]) -> Result<(Book, String, usize), String> {
    let mut b = book.clone();
    let mut first = None;
    let mut count = 0;
    for r in rows.iter().filter(|r| r.selected) {
        let port: u16 = r.port.trim().parse().map_err(|_| "Enter an SSH port from 1 to 65535.".to_string())?;
        let d = Device { alias: r.alias.trim().into(), user: r.user.trim().into(), name: r.ad.name.clone(), lan: r.address.to_string(), port, ..Default::default() };
        d.validate().map_err(|e| format!("{}: {e}", r.ad.name))?;
        if b.devices.iter().any(|other| other.alias.eq_ignore_ascii_case(&d.alias)) {
            return Err(format!("Alias {} already exists. Choose a different alias.", d.alias));
        }
        if b.devices.iter().any(|other| other.port == port &&
            [other.lan.as_str(), other.host.as_str()].iter().any(|addr| addr.parse::<IpAddr>().ok() == Some(r.address))) {
            return Err(format!("{}:{} is already saved. Edit that device from the book instead.", r.address, port));
        }
        if first.is_none() { first = Some(d.alias.clone()); }
        b.devices.push(d);
        count += 1;
    }
    Ok((b, first.ok_or("Select a LAN device to add.")?, count))
}

impl App {
    pub(super) fn lan_window(&mut self, ctx: &egui::Context) {
        let lan = &mut self.lan;
        if let Some((rx, stop)) = &lan.worker {
            match rx.try_recv() {
                Ok(result) => {
                    let result = if stop.load(Ordering::Relaxed) { Ok(Outcome::Cancelled) } else { result };
                    lan.worker = None;
                    if lan.open {
                        match result {
                            Ok(Outcome::Completed(rows)) => {
                                lan.rows = rows.into_iter().map(Row::new).collect();
                                lan.hint = if lan.rows.is_empty() {"No SSH advertisements found. You can still add a device manually."} else {"Review the advertised address and port, then enter your SSH username."}.into();
                            }
                            Ok(Outcome::Cancelled) => { lan.hint = "Search cancelled.".into(); }
                            Err(_) => { lan.hint = "Could not browse for SSH advertisements on that adapter. Check the adapter or add the device manually.".into(); }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    lan.hint = "LAN search stopped before completing.".into();
                    lan.worker = None;
                }
            }
        }
        if !lan.open { return; }
        let mut open = true;
        let mut start = false;
        let mut add = false;
        egui::Window::new("Find devices on LAN").open(&mut open).default_width(590.).show(ctx, |ui| {
            ui.label("Looks for SSH advertisements on one local adapter. It does not scan ports or connect.");
            ui.weak("Not every device advertises SSH. Confirm an address belongs to your device before connecting.");
            if self.path.is_some() {
                ComboBox::from_id_salt("lan-adapter").selected_text(lan.adapter.as_deref().unwrap_or("Choose a Wi-Fi or Ethernet adapter")).show_ui(ui, |ui| {
                    for a in &lan.adapters {
                        let addresses = a.addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
                        ui.selectable_value(&mut lan.adapter, Some(a.name.clone()), format!("{} ({addresses})", a.name));
                    }
                });
                ui.horizontal(|ui| {
                    start = ui.add_enabled(lan.worker.is_none() && lan.adapter.is_some(), egui::Button::new("Find advertised SSH services")).clicked();
                    if lan.worker.is_some() {
                        ui.spinner();
                        if ui.button("Cancel").clicked() { lan.cancel(); }
                    }
                });
            }
            ui.label(&lan.hint);
            ScrollArea::vertical().max_height(330.).show(ui, |ui| {
                for (i, r) in lan.rows.iter_mut().enumerate() {
                    ui.separator();
                    ui.checkbox(&mut r.selected, format!("{} ({})", r.ad.name, r.ad.hostname));
                    ui.monospace(format!("{}  |  advertised port {}", r.address, r.ad.port));
                    if r.selected {
                        ui.horizontal_wrapped(|ui| {
                            ComboBox::from_id_salt(("lan-address", i)).selected_text(r.address.to_string()).show_ui(ui, |ui| {
                                for a in &r.ad.addresses { ui.selectable_value(&mut r.address, *a, a.to_string()); }
                            });
                            ui.label("Alias"); ui.add(TextEdit::singleline(&mut r.alias).desired_width(100.));
                            ui.label("SSH user"); ui.add(TextEdit::singleline(&mut r.user).hint_text("Required").desired_width(90.));
                            ui.label("SSH port"); ui.add(TextEdit::singleline(&mut r.port).desired_width(48.));
                        });
                    }
                }
            });
            let count = lan.rows.iter().filter(|r| r.selected).count();
            if count > 0 { add = ui.button(format!("Add {count} reviewed devices")).clicked(); }
        });
        lan.open = open;
        if !open {
            lan.cancel();
            lan.rows.clear();
            return;
        }
        if start && let Some(adapter) = lan.adapter.clone() {
            lan.rows.clear();
            lan.hint = "Looking for SSH advertisements...".into();
            let stop = Arc::new(AtomicBool::new(false));
            let cancel = Arc::clone(&stop);
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || { let _ = tx.send(lan_discovery::browse(&adapter, &cancel)); });
            lan.worker = Some((rx, stop));
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if add {
            let result = apply(&self.book, &self.lan.rows)
                .and_then(|(book, alias, count)| { self.replace(book)?; Ok((alias, count)) });
            match result {
                Ok((alias, count)) => {
                    self.selected = Some(alias);
                    self.lan.open = false;
                    self.lan.rows.clear();
                    self.report(Ok(format!("Added {count} reviewed LAN devices.")));
                }
                Err(e) => self.lan.hint = e,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn review_requires_manual_user_and_stores_only_lan_route() {
        let mut state = LanState::default();
        state.show(true);
        let row = &mut state.rows[0];
        assert!(row.user.is_empty());
        assert!(!row.selected);
        row.selected = true;
        assert!(apply(&Book::default(), &state.rows).is_err());
        state.rows[0].user = "alice".into();
        let (book, _, count) = apply(&Book::default(), &state.rows).unwrap();
        assert_eq!(count, 1);
        assert_eq!(book.devices[0].lan, "192.0.2.34");
        assert!(book.devices[0].tailscale.is_empty());
        assert_eq!(book.devices[0].port, 2222);
        assert!(apply(&book, &state.rows).is_err());
        state.rows[0].alias = "another".into();
        assert!(apply(&book, &state.rows).is_err());
    }
}
