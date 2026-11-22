#![cfg_attr(all(target_os="windows", not(debug_assertions)), windows_subsystem="windows")]
mod book;
mod csv_review;
mod theme;
mod discovery;
mod lan_discovery;
mod lan_review;
mod device_art;
mod terminal;
mod terminal_ui;
mod external_terminal;
mod ssh_setup;
use book::{Book, Device, DeviceKind, Revision, Route, SaveError};
use eframe::egui::{self, ComboBox, RichText, ScrollArea, TextEdit};
use std::{collections::{BTreeMap, HashSet}, path::PathBuf};

const TITLE: &str = "DrakOps | Home SysOps";
const SUBTITLE: &str = "Home SysOps";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
struct Edit { original: Option<Device>, device: Device, port: String }
struct App {
    header_icon: Option<egui::TextureHandle>,
    show_about: bool,
    book: Book, path: Option<PathBuf>, revision: Revision, inventory_conflict: bool, locked: bool, error: Option<String>, status: String,
    selected: Option<String>, query: String, routes: BTreeMap<String,(Route,String)>,
    editor: Option<Edit>, show_import: bool, import_path: String, import_picker: Option<std::sync::mpsc::Receiver<Option<PathBuf>>>, review: Option<Vec<Device>>, csv_review: csv_review::CsvReview,
    lan: lan_review::LanState,
    terminal: terminal_ui::TerminalUi,
    setup: Option<ssh_setup::Setup>,
    key_install_attempts: ssh_setup::InstallAttempts,
    removing: Option<Device>,
    last_removed: Option<(usize,Device)>,
    show_discovery: bool, discovery_hint: String, rehearsal_loaded: bool,
    discovery_rx: Option<std::sync::mpsc::Receiver<Result<Vec<discovery::Suggestion>,String>>>,
    discovery_rows: Vec<(discovery::Suggestion,Device,String,bool)>,
}

fn demo_book() -> Book {
    Book { devices: vec![
        Device { alias: "fedora".into(), name: "Framework Desktop".into(), kind: DeviceKind::Desktop, lan: "192.0.2.16".into(), tailscale: "framework-desktop.example".into(), user: "alex".into(), tags: "Workstations".into(), notes: "Example build machine. These demonstration addresses are not real devices.".into(), ..Device::default() },
        Device { alias: "pve".into(), name: "Proxmox Mini".into(), kind: DeviceKind::Server, lan: "192.0.2.20".into(), tailscale: "proxmox-mini.example".into(), user: "alex".into(), tags: "Lab".into(), notes: "Example virtual machine host.".into(), ..Device::default() },
        Device { alias: "laptop".into(), name: "Travel Laptop".into(), kind: DeviceKind::Laptop, lan: "192.0.2.40".into(), user: "alex".into(), ..Device::default() },
        Device { alias: "sensor".into(), name: "Workshop Sensor".into(), kind: DeviceKind::Iot, lan: "192.0.2.83".into(), user: "alex".into(), ..Device::default() },
    ], ..Book::default() }
}
fn select_csv_path(current:&mut String,result:Option<PathBuf>)->bool {
    if let Some(path)=result.and_then(|p|p.into_os_string().into_string().ok()) && path!=*current {
        *current=path;
        return true;
    }
    false
}

fn connection_routes(device:&Device)->Vec<Route>{
    let mut routes=Vec::new();
    for route in Route::ALL{
        if device.ssh_args(route).is_ok() && !routes.iter().any(|other|device.address(*other).eq_ignore_ascii_case(device.address(route))) {
            routes.push(route);
        }
    }
    routes
}
fn connection_route(routes:&BTreeMap<String,(Route,String)>,device:&Device)->Route{
    routes.get(&device.alias).filter(|(route,host)|device.address(*route).eq_ignore_ascii_case(host) && connection_routes(device).contains(route))
        .map(|(route,_)|*route).unwrap_or_default()
}
fn endpoint(device:&Device,route:Route)->String{
    let host=device.address(route);
    let shown=if host.contains(':'){format!("[{host}]")}else{host.to_owned()};
    format!("{}@{}:{}",device.user,shown,device.port)
}
impl App {
    fn new(cc: &eframe::CreationContext<'_>, demo: bool) -> Self {
        let path = if demo { None } else {directories::ProjectDirs::from("net","HomeOps","HomeOps").map(|d|d.data_local_dir().join("device-book.json"))};
        let unreadable = !demo && path.is_none();
        let (book, revision, error) = if demo { (demo_book(), Revision::default(), None) } else if let Some(p)=&path {
            match Book::read(p) {
                Ok((book, revision)) => (book, revision, None),
                Err(e) => (Book::default(),Revision::default(),Some(format!("Existing inventory could not be read and will not be overwritten: {e}"))),
            }
        } else { (Book::default(),Revision::default(),Some("Could not locate your operating system's data folder.".into())) };
        theme::apply(&cc.egui_ctx,&book.theme);
        let header_icon=eframe::icon_data::from_png_bytes(include_bytes!("../assets/brand/drakops.png")).ok()
            .map(|icon|cc.egui_ctx.load_texture(
                "drakops-header-icon",
                egui::ColorImage::from_rgba_unmultiplied([icon.width as usize,icon.height as usize],&icon.rgba),
                egui::TextureOptions::LINEAR,
            ));

        let home = directories::BaseDirs::new().map(|d| d.home_dir().to_owned());
        let import_path = home.as_ref().map(|p|p.join(".ssh").join("quick-connect").join("hosts.csv").display().to_string()).unwrap_or_default();
        Self {
            header_icon,
            show_about:false,
            selected:book.devices.first().map(|d|d.alias.clone()), locked:unreadable||error.is_some(),
            book,path,revision,inventory_conflict:false,error,status:if demo { "Demonstration profile · not saved".into() } else { String::new() },
            query:String::new(),routes:Default::default(),editor:None,
            show_import:false,import_path,import_picker:None,review:None,csv_review:Default::default(),
            lan: Default::default(),
            terminal: Default::default(),
            setup: None,
            key_install_attempts: Default::default(),
            removing: None,
            last_removed: None,
            show_discovery:false,discovery_hint:String::new(),rehearsal_loaded:false,
            discovery_rx:None,discovery_rows:Vec::new()
        }
    }
    fn report(&mut self, result: Result<String,String>) {
        match result {
            Ok(s)=>{self.error=None;self.status=s;},
            Err(e)=>{self.status.clear();self.error=Some(e);},
        }
    }
    fn replace(&mut self, book: Book) -> Result<(),String> {
        if self.locked { return Err("Inventory is read-only until the data file is recovered.".into()); }
        if self.inventory_conflict { return Err(SaveError::Conflict.to_string()); }
        if let Some(p)=&self.path {
            match book.save(p,&self.revision) {
                Ok(revision) => self.revision=revision,
                Err(e) => {
                    self.inventory_conflict=matches!(e,SaveError::Conflict);
                    return Err(e.to_string());
                }
            }
        }
        self.book=book; Ok(())
    }
    fn reload_inventory(&mut self, ctx:&egui::Context) -> Result<String,String> {
        let path=self.path.as_ref().ok_or("No saved inventory is available.")?;
        let (book,revision)=Book::read(path)?;
        self.selected=self.selected.take().filter(|alias|book.devices.iter().any(|d|&d.alias==alias))
            .or_else(||book.devices.first().map(|d|d.alias.clone()));
        self.book=book;
        self.revision=revision;
        self.inventory_conflict=false;
        self.routes.clear();
        self.setup=None; // Recheck the endpoint against the current inventory before key setup.
        theme::apply(ctx,&self.book.theme);
        Ok("Saved inventory reloaded. Open drafts and CSV reviews are still available.".into())
    }
    fn apply_theme(&mut self, ctx:&egui::Context, id:String) {
        let mut b=self.book.clone();
        b.theme=id.clone();
        let r=self.replace(b).map(|_| {theme::apply(ctx,&id); "Theme applied.".into()});
        self.report(r);
    }
    fn open_editor(&mut self, device:Device, original:Option<Device>) {
        self.editor=Some(Edit{port:device.port.to_string(),device,original});
    }
    fn sidebar(&mut self, context:&egui::Context) {
        egui::SidePanel::left("devices")
            .default_width(318.).width_range(230.0..=400.0).show(context,|ui|{
                ui.spacing_mut().item_spacing = egui::vec2(8., 7.);
                ui.heading("Devices");
                ui.add_sized([ui.available_width(),33.],TextEdit::singleline(&mut self.query).hint_text("Search devices, addresses, or tags"));
                ui.add_space(8.);
                ui.horizontal_wrapped(|ui|{
                    if theme::primary_button(ui,"+ Add device",!self.locked,egui::vec2(116.,32.)).clicked() {self.open_editor(Device::default(),None);}
                    if ui.add_enabled(!self.locked,egui::Button::new("Import CSV").min_size(egui::vec2(88.,32.))).clicked() {self.show_import=true;self.review=None;}
                });
                ui.horizontal_wrapped(|ui| { if ui.add_enabled(!self.locked,egui::Button::new("Find on Tailscale").min_size(egui::vec2(124.,30.))).clicked(){
                    self.show_discovery=true;
                    if self.path.is_none() && !self.rehearsal_loaded {
            let bytes=br#"{"BackendState":"Running","Peer":{"a":{"ID":"pve","DNSName":"proxmox-mini.example.","HostName":"Proxmox Mini","TailscaleIPs":["100.64.0.2"],"Online":true},"b":{"ID":"fixture","DNSName":"backup-box.example.","HostName":"Backup Box","TailscaleIPs":["100.64.0.3"]}}}"#;
            if let Ok(rows)=discovery::parse(bytes){
                self.discovery_rows=rows.into_iter().map(|s|{
                    let row=Device{alias:s.alias.clone(),name:s.name.clone(),tailscale:s.address.clone(),..Device::default()};
                    (s,row,String::from("22"),false)
                }).collect();
                self.discovery_hint="Showing synthetic sample peers.".into();
                self.rehearsal_loaded=true;
            }
                    }
                }
                if ui.add_enabled(!self.locked,egui::Button::new("Find on LAN").min_size(egui::vec2(95.,30.))).clicked(){self.lan.show(self.path.is_none());} }); ui.separator();
                let shown: Vec<_> = self.book.devices.iter().filter(|d|d.matches(&self.query)).collect();
                ui.add_space(9.);
                ScrollArea::vertical().id_salt("device-list").show(ui,|ui|{
                    if shown.is_empty() {
                        ui.weak(if self.book.devices.is_empty() {"No devices yet. Add one or import your connect list."} else {"No matching devices."});
                    }
                    for d in shown {
                        ui.horizontal(|ui|{
                            device_art::illustration(ui, d.kind, 33.);
                            ui.vertical(|ui| {
                                if ui.add(egui::Button::selectable(self.selected.as_deref()==Some(&d.alias), d.title()).min_size(egui::vec2(112.,28.)).wrap_mode(egui::TextWrapMode::Truncate)).on_hover_text(d.title()).clicked() {
                                    self.selected=Some(d.alias.clone());
                                }
                                let caption=if d.tags.is_empty() { format!("{} | {}", d.kind.label(), d.alias) } else { format!("{} | {}", d.kind.label(), d.tags) }; ui.add(egui::Label::new(RichText::new(&caption).weak()).truncate()).on_hover_text(caption);
                            });
                        });
                        ui.add_space(4.);
                    }
                });
            });
    }
    fn details(&mut self, ctx:&egui::Context) {
        egui::CentralPanel::default().show(ctx,|ui| {
            ui.add_space(8.);
            let Some(d)=self.book.devices.iter().find(|d|Some(d.alias.as_str())==self.selected.as_deref()).cloned() else {
                ui.heading("Your connections, in one place");
                ui.weak("Save a device or import an existing hosts.csv. Nothing connects until you ask.");
                ui.add_space(12.);
                if let Some(p)=&self.path {ui.collapsing("Inventory storage",|ui|{ui.monospace(p.display().to_string());});}
                return;
            };
            ScrollArea::vertical().show(ui,|ui|{
                ui.spacing_mut().item_spacing = egui::vec2(10., 9.);
                ui.horizontal_wrapped(|ui| {
                    device_art::illustration(ui,d.kind,86.);
                    ui.vertical(|ui| {
                        ui.weak(d.kind.label());
                        ui.add(egui::Label::new(RichText::new(d.title()).size(25.).strong()).wrap());
                        if !d.tags.is_empty() {ui.weak(&d.tags);}
                    });
                });
                ui.add_space(9.);
                let available=connection_routes(&d);
                let mut route=connection_route(&self.routes,&d);
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!self.locked,egui::Button::new("Edit device").min_size(egui::vec2(88.,31.))).clicked() {self.open_editor(d.clone(),Some(d.clone()));}
                    let busy=self.terminal.refers_to(&d);
                    if ui.add_enabled(!self.locked && !busy,egui::Button::new("Remove device").min_size(egui::vec2(108.,31.)))
                        .on_disabled_hover_text("Disconnect this device's embedded SSH session before removing it.").clicked(){
                        self.removing=Some(d.clone());
                    }
                });
                ui.separator();
                egui::Grid::new("details").spacing([18.,8.]).num_columns(2).show(ui,|ui| {
                    let port=d.port.to_string();
                    for (label,value) in [("Alias", d.alias.as_str()),("SSH username",d.user.as_str()),("Port",port.as_str()),("LAN address",d.lan.as_str()),("Tailscale address",d.tailscale.as_str())] {
                        ui.weak(label);ui.monospace(if value.is_empty() {"Not set"}else{value});ui.end_row();
                    }
                });
                if !d.notes.is_empty() {ui.add_space(10.);ui.label(RichText::new("Notes").strong());ui.label(&d.notes);}
                ui.add_space(12.);
                if d.ssh_args(route).is_ok(){
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("Connect to");
                        if available.len()>1 {
                            let previous=route;
                            ComboBox::from_id_salt(("device-route",&d.alias)).selected_text("Change route").show_ui(ui,|ui|{
                                for candidate in &available {
                                    ui.selectable_value(&mut route,*candidate,format!("{} · {}",candidate.label(),endpoint(&d,*candidate)));
                                }
                            });
                            if route!=previous {
                                if route==Route::Saved {self.routes.remove(&d.alias);}
                                else {self.routes.insert(d.alias.clone(),(route,d.address(route).to_owned()));}
                            }
                        }
                        ui.label(RichText::new(format!("{} · {}",endpoint(&d,route),route.label())).monospace());
                    });
                }
                if let Ok(command)=d.ssh(route) {
                    ui.label(RichText::new(command.clone()).monospace());
                    ui.add_space(5.);
                    ui.horizontal_wrapped(|ui|{
                        let live=self.path.is_some() && !self.locked && self.terminal.can_offer_new_tab(&d,route);
                        let (connect,options)=if live {
                            let (main,options)=theme::connect_split(ui);
                            (main,Some(options))
                        } else {
                            (theme::primary_button(ui,if self.path.is_some() {"Connect"} else {"Preview terminal"},!self.locked,egui::vec2(110.,33.)),None)
                        };
                        if connect.on_hover_text("Embedded SSH uses an already verified OpenSSH host. For a new host use external SSH or copy the command.").clicked() {
                            self.terminal.start(&d,route,self.path.is_none());
                        }
                        if let Some(options)=options{
                            egui::Popup::menu(&options).show(|ui|{
                                let space=self.terminal.has_tab_space();
                                if ui.add_enabled(space,egui::Button::new("Open new SSH tab")).clicked(){
                                    self.terminal.start_new_tab(&d,route);
                                    ui.close();
                                }
                                if !space{ui.weak("Close a tab to open another session.");}
                            });
                        }
                        if ui.add_enabled(self.path.is_some(),egui::Button::new("Open in external terminal").min_size(egui::vec2(156.,33.)))
                            .on_hover_text("Use OpenSSH in a separate window. Verify the fingerprint before accepting a first-time host.").clicked() {
                            self.report(external_terminal::open(&d,route).map(|_|"External terminal launch requested. Follow the prompts there.".into()));
                        }
                        if ui.add_enabled(self.path.is_some() && !self.locked,egui::Button::new("Set up SSH").min_size(egui::vec2(97.,33.))).clicked(){
                            self.setup=Some(ssh_setup::Setup::new(d.clone(),route));
                        }

                        if ui.add(egui::Button::new("Copy command").min_size(egui::vec2(105.,32.))).clicked() {
                            ctx.copy_text(command.clone());self.report(Ok("SSH command copied.".into()));
                        }
                        if ui.add(egui::Button::new("Copy details").min_size(egui::vec2(90.,32.))).clicked() {
                            ctx.copy_text(format!("Device: {}\nAlias: {}\nHost: {}\nSSH username: {}\nPort: {}\nRoute: {}",
                              d.title(),d.alias,d.address(route),d.user,d.port,route.label()));
                            self.report(Ok("Connection details copied.".into()));
                        }
                    });
                }
            });
        });
    }
    fn remove_dialog(&mut self,ctx:&egui::Context){
        let Some(expected)=self.removing.take() else{return;};
        let busy=self.terminal.refers_to(&expected);
        let response=egui::Modal::new(egui::Id::new("remove-device-confirm")).show(ctx,|ui|{
            ui.set_max_width(430.);
            ui.heading("Remove this device?");
            ui.label(format!("{} ({})",expected.title(),expected.alias));
            ui.label("This only removes the saved DrakOps entry. It does not delete keys, change the remote computer, or close an external terminal.");
            ui.weak("You can undo the most recent removal before you close DrakOps.");
            if busy{ui.label("Disconnect this device's embedded SSH session first.");}
            let mut remove=false;
            let mut cancel=false;
            ui.horizontal(|ui|{
                cancel=ui.button("Cancel").clicked();
                remove=ui.add_enabled(!self.locked&&!busy,egui::Button::new("Remove device")).clicked();
            });
            (remove,cancel)
        });
        let (remove,cancel)=response.inner;
        if remove {
            let mut book=self.book.clone();
            let result=book.remove_reviewed(&expected).and_then(|removed|{self.replace(book)?;Ok(removed)});
            match result {
                Ok(removed)=>{
                    self.routes.remove(&expected.alias);
                    if self.selected.as_deref()==Some(&expected.alias){
                        self.selected=self.book.devices.get(removed.0.min(self.book.devices.len().saturating_sub(1))).map(|d|d.alias.clone());
                    }
                    if self.editor.as_ref().is_some_and(|edit|edit.original.as_ref().is_some_and(|original|original.alias==expected.alias)){self.editor=None;}
                    if self.setup.as_ref().is_some_and(|setup|setup.refers_to(&expected)){self.setup=None;}
                    self.terminal.forget_closed(&expected);
                    self.last_removed=Some(removed);
                    self.report(Ok(format!("Removed {} from DrakOps.",expected.title())));
                }
                Err(error)=>self.report(Err(error)),
            }
        } else if !cancel && !response.should_close(){self.removing=Some(expected);}
    }

    fn undo_remove(&mut self){
        let Some((index,device))=self.last_removed.clone() else{return;};
        let mut book=self.book.clone();
        let result=book.restore_removed(index,device.clone()).and_then(|_|self.replace(book));
        match result {
            Ok(())=>{
                self.last_removed=None;
                self.selected=Some(device.alias.clone());
                self.report(Ok(format!("Restored {}.",device.title())));
            }
            Err(error)=>self.report(Err(error)),
        }
    }

    fn edit_window(&mut self, ctx:&egui::Context) {
        let Some(mut edit)=self.editor.take() else {return;};
        let mut open=true;let mut save=false;let mut cancel=false;
        egui::Window::new(if edit.original.is_some(){"Edit device"}else{"Add device"})
            .open(&mut open).default_width(460.).show(ctx,|ui|{
                egui::Grid::new("edit-form").num_columns(2).spacing([12.,7.]).show(ui,|ui|{
                    ui.label("Device type"); ComboBox::from_id_salt("device-kind").selected_text(edit.device.kind.label()).show_ui(ui, |ui| {for kind in DeviceKind::ALL {ui.selectable_value(&mut edit.device.kind,kind,kind.label());}}); ui.end_row();
                    for (label, field) in [("Display name",&mut edit.device.name),("Short alias *",&mut edit.device.alias),("Saved host",&mut edit.device.host),("LAN address",&mut edit.device.lan),("Tailscale name/IP",&mut edit.device.tailscale),("SSH username *",&mut edit.device.user),("SSH port *",&mut edit.port),("Tags",&mut edit.device.tags)] {
                        ui.label(label);ui.add(TextEdit::singleline(field).desired_width(285.));ui.end_row();
                    }
                });
                ui.weak("Set at least one address. The saved host is used first by default.");
                ui.label("Notes");
                ui.add(TextEdit::multiline(&mut edit.device.notes).desired_rows(3).desired_width(f32::INFINITY));
                if let Some(e)=&self.error {ui.colored_label(theme::roles(ctx).error,e);}
                ui.horizontal(|ui|{save=ui.button("Save device").clicked();cancel=ui.button("Cancel").clicked();});
            });
        if save {
            for s in [&mut edit.device.alias,&mut edit.device.name,&mut edit.device.host,&mut edit.device.lan,&mut edit.device.tailscale,&mut edit.device.user,&mut edit.device.tags] {
                *s=s.trim().into();
            }
            let result=(|| {
                edit.device.port=edit.port.trim().parse().map_err(|_|"SSH port must be 1–65535.")?;
                edit.device.validate()?;
                if let Some(original)=&edit.original && !self.book.devices.contains(original) {
                    return Err("This device changed while you were editing. Your draft is still open. Copy any edits you want to keep before starting a new edit.".into());
                }
                if self.book.devices.iter().any(|d| Some(d)!=edit.original.as_ref() && d.alias.eq_ignore_ascii_case(&edit.device.alias)) { return Err("That alias already belongs to a device.".into()); }
                let mut b=self.book.clone();
                let mut changed_destination=false;
                if let Some(original)=&edit.original {
                    let entry=b.devices.iter_mut().find(|d|&**d==original).ok_or("The original device is no longer available.")?;
                    changed_destination=(&entry.alias,&entry.host,&entry.lan,&entry.tailscale,&entry.user,entry.port)!=
                        (&edit.device.alias,&edit.device.host,&edit.device.lan,&edit.device.tailscale,&edit.device.user,edit.device.port);
                    *entry=edit.device.clone();
                }
                else {b.devices.push(edit.device.clone());}
                self.replace(b)?;
                if let Some(old)=&edit.original {
                    if changed_destination {
                        self.routes.remove(&old.alias);
                    }
                }
                self.selected=Some(edit.device.alias.clone());
                Ok("Device saved.".into())
            })();
            let keep=result.is_err();
            self.report(result);
            if keep {self.editor=Some(edit);}
        } else if open && !cancel {self.editor=Some(edit);}
    }
    fn import_window(&mut self, ctx:&egui::Context) {
        if let Some(picker)=&self.import_picker {
            match picker.try_recv() {
                Ok(path)=>{ self.import_picker=None; if self.show_import && select_csv_path(&mut self.import_path,path){self.review=None;self.csv_review.invalidate();} }
                Err(std::sync::mpsc::TryRecvError::Empty)=>ctx.request_repaint_after(std::time::Duration::from_millis(80)),
                Err(_)=>{self.import_picker=None; if self.show_import {self.report(Err("CSV file chooser unavailable. Enter the CSV path instead.".into()));}}
            }
        }
        if !self.show_import {
            self.csv_review.invalidate();
            let _=self.csv_review.poll();
            return;
        }
        let mut open=true;let mut review=false;let mut apply=false;let mut browse=false;
        egui::Window::new("Import connect devices").open(&mut open).default_width(485.).show(ctx,|ui|{
            browse=ui.add_enabled(self.import_picker.is_none(),egui::Button::new("Browse for CSV…")).clicked();
            if self.import_picker.is_some(){ui.spinner();}
            ui.label("Or enter a CSV path:");
            if ui.add(TextEdit::singleline(&mut self.import_path).desired_width(f32::INFINITY)).changed(){self.review=None;self.csv_review.invalidate();}
            ui.weak("Expected columns: alias,hostname,user,port. Existing aliases are not changed.");
            review=ui.add_enabled(!self.csv_review.busy() && !self.import_path.trim().is_empty(),egui::Button::new("Review CSV")).clicked();
            if self.csv_review.current(){
                ui.horizontal(|ui|{
                    ui.spinner();
                    ui.weak("Reading the CSV without opening any connections.");
                    if ui.button("Cancel CSV review").clicked(){self.csv_review.invalidate();}
                });
            }
            else if self.csv_review.busy(){ui.weak("The previous CSV read is still finishing.");}
            if let Some(rows)=&self.review {
                let saved:HashSet<_>=self.book.devices.iter().map(|d|d.alias.to_ascii_lowercase()).collect();
                let duplicate=|row:&Device|saved.contains(&row.alias.to_ascii_lowercase());
                let additions=rows.iter().filter(|r|!duplicate(r)).count();
                ui.label(format!("{additions} new · {} existing skipped",rows.len()-additions));
                ScrollArea::vertical().max_height(190.).show_rows(ui,ui.text_style_height(&egui::TextStyle::Body),rows.len(),|ui,range|{
                    for row in &rows[range] {
                        let label=format!("{} · {}{}",row.alias,endpoint(row,Route::Saved),if duplicate(row) {" (already exists)"}else{""});
                        ui.add(egui::Label::new(&label).truncate()).on_hover_text(label);
                    }
                });
                let fits=self.book.devices.len()+additions<=book::MAX_DEVICES;
                if !fits {ui.weak("This import would exceed the inventory's device limit. Review a smaller CSV.");}
                apply=ui.add_enabled(additions>0 && fits,egui::Button::new("Add new devices")).clicked();
            }
            if let Some(e)=&self.error {ui.colored_label(theme::roles(ctx).error,e);}
        });
        self.show_import=open;
        if !open {
            self.import_picker=None;
            self.csv_review.invalidate();
            let _=self.csv_review.poll();
            return;
        }
        if open && browse {
            let candidate=PathBuf::from(self.import_path.trim());
            let (tx,rx)=std::sync::mpsc::channel();
            match std::thread::Builder::new().name("homeops-csv-picker".into()).spawn(move||{
                let dir=candidate.parent().filter(|p|p.is_absolute()&&p.is_dir()).map(|p|p.to_owned()).or_else(||{
                    let home=directories::BaseDirs::new()?.home_dir().to_owned();
                    let legacy=home.join(".ssh").join("quick-connect");
                    Some(if legacy.is_dir(){legacy}else{home})
                });
                let mut dialog=rfd::FileDialog::new().set_title("Choose a device CSV").add_filter("CSV file",&["csv"]);
                if let Some(dir)=dir {dialog=dialog.set_directory(dir);}
                let _=tx.send(dialog.pick_file());
            }){
                Ok(_)=>{self.import_picker=Some(rx);ctx.request_repaint_after(std::time::Duration::from_millis(80));}
                Err(_)=>self.report(Err("CSV file chooser unavailable. Enter the CSV path instead.".into())),
            }
        }
        if review {
            self.review=None;
            if let Err(e)=self.csv_review.start(&self.import_path){self.report(Err(e));}
        }
        if let Some(result)=self.csv_review.poll(){
            match result { Ok(rows)=>{self.review=Some(rows);self.report(Ok("Review the SSH account and address before adding devices.".into()));},Err(e)=>self.report(Err(e)) }
            ctx.request_repaint();
        } else if self.csv_review.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if apply && let Some(rows)=self.review.clone() {
            let mut b=self.book.clone();let before=b.devices.len();
            let mut aliases:HashSet<_>=b.devices.iter().map(|d|d.alias.to_ascii_lowercase()).collect();
            b.devices.extend(rows.into_iter().filter(|row|aliases.insert(row.alias.to_ascii_lowercase())));
            let added=b.devices.len()-before;
            let r=self.replace(b);if r.is_ok(){self.show_import=false;self.review=None;if self.selected.is_none(){self.selected=self.book.devices.first().map(|d|d.alias.clone());}}
            self.report(r.map(|_|format!("{added} devices added.")));
        }
    }
    fn discovery_window(&mut self, ctx: &egui::Context) {
        if let Some(rx)=&self.discovery_rx {
            match rx.try_recv() {
                Ok(Ok(rows)) => {
                    self.discovery_rows=rows.into_iter().map(|s| {
                        let row=Device{alias:s.alias.clone(),name:s.name.clone(),tailscale:s.address.clone(),..Device::default()};
                        (s,row,String::from("22"),false)
                    }).collect();
                    self.discovery_rx=None;
                    self.discovery_hint="Review the SSH username and port for each selected device.".into();
                    self.rehearsal_loaded=true;
                }
                Ok(Err(e)) => {self.discovery_hint=e;self.discovery_rx=None;}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {self.discovery_hint="Could not read local Tailscale status.".into();self.discovery_rx=None;}
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(120)),
            }
        }
        if !self.show_discovery {return;}
        let mut open=true;
        let mut start=false;
        let mut apply=false;
        egui::Window::new("Find devices on Tailscale").open(&mut open).default_width(650.).show(ctx,|ui|{
            ui.label("Reads the local Tailscale device list. No scan, SSH test, or changes.");
            ui.weak("Tailscale status is not proof that SSH is enabled.");
            start=ui.add_enabled(self.discovery_rx.is_none() && self.path.is_some(),egui::Button::new("Read Tailscale device list")).clicked();
            if self.path.is_none() {ui.weak("Demo uses synthetic peers; it cannot read your device list.");}
            if !self.discovery_hint.is_empty(){ui.label(&self.discovery_hint);}
            if self.discovery_rx.is_some(){ui.spinner();}
            if !self.discovery_rows.is_empty(){
                ScrollArea::vertical().max_height(330.).show(ui,|ui|{
                    for (s,device,port,tick) in &mut self.discovery_rows{
                        ui.separator();
                        if let Some(existing)=discovery::existing(&self.book.devices,s){
                            *tick=false;
                            ui.label(format!("{} · {} · Already saved as {}",s.name,s.address,existing.alias));
                            continue;
                        }
                        ui.checkbox(tick,format!("{} · {}",s.name,if s.online {"Online in Tailscale"}else{"Offline in Tailscale"}));
                        ui.monospace(&s.address);
                        if *tick {
                            ui.horizontal_wrapped(|ui|{
                                ui.label("Alias");
                                ui.add(TextEdit::singleline(&mut device.alias).desired_width(109.));
                                ui.label("SSH user");
                                ui.add(TextEdit::singleline(&mut device.user).hint_text("Required").desired_width(100.));
                                ui.label("SSH port");
                                ui.add(TextEdit::singleline(port).desired_width(47.));
                            });
                        }
                    }
                });
                let count=self.discovery_rows.iter().filter(|row|row.3).count();
                apply=ui.add_enabled(count>0,egui::Button::new(format!("Add {count} reviewed devices"))).clicked();
            }else if self.rehearsal_loaded && self.discovery_rx.is_none(){ui.weak("No peers with Tailscale addresses were found.");}
        });
        self.show_discovery=open;
        if start {
            self.discovery_rows.clear();
            self.rehearsal_loaded=false;
            let (tx,rx)=std::sync::mpsc::channel();
            std::thread::spawn(move || {let _=tx.send(discovery::discover());});
            self.discovery_hint="Reading local Tailscale status…".into();
            self.discovery_rx=Some(rx);
        }
        if apply {
            let result=(|| {
                let mut b=self.book.clone();
                let mut first=None;
                let mut n=0;
                for (s,d,port,tick) in &self.discovery_rows {
                    if !*tick {continue;}
                    if discovery::existing(&b.devices,s).is_some(){continue;}
                    let mut d=d.clone();
                    d.alias=d.alias.trim().into();d.user=d.user.trim().into();
                    d.port=port.trim().parse().map_err(|_|format!("{}: enter an SSH port from 1–65535.",s.name))?;
                    d.validate().map_err(|e|format!("{}: {e}",s.name))?;
                    if b.devices.iter().any(|other|other.alias.eq_ignore_ascii_case(&d.alias)){
                        return Err(format!("Alias {} already exists. Choose a different alias.",d.alias));
                    }
                    if first.is_none(){first=Some(d.alias.clone());}
                    n+=1;
                    b.devices.push(d);
                }
                if n==0{return Err("All selected peers are already in your inventory.".into());}
                self.replace(b)?;
                self.selected=first;
                self.show_discovery=false;
                self.discovery_rows.clear();
                Ok(format!("Added {n} reviewed Tailscale devices."))
            })();
            match result{
                Ok(message)=>self.report(Ok(message)),
                Err(e)=>self.discovery_hint=e,
            }
        }
    }

}
impl eframe::App for App {
    fn update(&mut self,ctx:&egui::Context,_frame:&mut eframe::Frame) {
        egui::TopBottomPanel::top("top").show(ctx,|ui|{
            ui.horizontal_wrapped(|ui| {
                ui.add_space(5.);
                ui.horizontal(|ui| {
                    if let Some(icon)=&self.header_icon {
                        ui.add(egui::Image::new((icon.id(),egui::vec2(38.,38.))).alt_text("DrakOps dragon icon"));
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new("DrakOps").size(25.).strong());
                        ui.weak(SUBTITLE);
                    });
                });
                ui.separator();
                ui.menu_button("Settings", |ui| {
                    let mut enabled=self.book.copy_on_select;
                    if ui.checkbox(&mut enabled,"Copy on select (terminal)").on_hover_text("Copy highlighted terminal text on mouse release.").changed(){
                        let mut book=self.book.clone();book.copy_on_select=enabled;
                        let result=self.replace(book).map(|_|"Terminal copy preference saved.".into());self.report(result);
                    }
                    ui.separator();
                    if ui.button("About").clicked(){
                        self.show_about=true;
                        ui.close();
                    }
                });
                ui.label("Theme");
                let mut selected=self.book.theme.clone();
                ComboBox::from_id_salt("theme").width(152.).selected_text(&theme::find(&selected).name).show_ui(ui,|ui|{
                    for (i,t) in theme::inventory().iter().enumerate(){
                        if i==0 || theme::inventory()[i-1].group!=t.group{ui.weak(&t.group);}
                        ui.selectable_value(&mut selected,t.id.clone(),&t.name);
                    }
                });
                if selected!=self.book.theme{self.apply_theme(ctx,selected);}
                if self.path.is_none() && !self.locked{ui.weak("Sample profile");}
            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx,|ui| {
            if self.inventory_conflict {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(theme::roles(ctx).error,SaveError::Conflict.to_string());
                    if ui.button("Reload saved inventory").clicked(){
                        let result=self.reload_inventory(ctx);
                        self.report(result);
                    }
                });
            }
            if let Some(e)=&self.error{ui.colored_label(theme::roles(ctx).error,e);}
            else if self.status.is_empty(){ui.weak("Local inventory · No keys or passwords stored");}
            else {ui.label(&self.status);}
            if self.last_removed.is_some() && ui.button("Undo last device removal").clicked(){self.undo_remove();}
        });
        self.terminal_panel(ctx,_frame); self.sidebar(ctx);self.details(ctx); if let Some(setup)=self.setup.take(){self.setup=setup.show(ctx,&mut self.key_install_attempts);}self.edit_window(ctx);self.import_window(ctx);self.discovery_window(ctx);self.lan_window(ctx);self.remove_dialog(ctx);
        if self.show_about {
            let mut open=self.show_about;
            egui::Window::new("About DrakOps").open(&mut open).resizable(false).collapsible(false)
                .show(ctx,|ui|{
                    ui.heading("DrakOps");
                    ui.label(SUBTITLE);
                    ui.label(format!("Version {APP_VERSION}"));
                });
            self.show_about=open;
        }
        self.terminal.finish_frame(ctx);
    }
}
#[cfg(windows)]
fn windows_graphics(options:&mut eframe::NativeOptions,software_only:bool,diagnostics:bool) {
    use eframe::{egui_wgpu::WgpuSetupCreateNew,wgpu};
    let mut setup=WgpuSetupCreateNew::default();
    setup.instance_descriptor.backends=wgpu::Backends::DX12;
    setup.instance_descriptor.backend_options.dx12.shader_compiler=wgpu::Dx12Compiler::Fxc;
    setup.native_adapter_selector=Some(std::sync::Arc::new(move |adapters,surface|{
        let usable=|a:&&wgpu::Adapter|surface.is_none_or(|s|a.is_surface_supported(s));
        let software=|a:&&wgpu::Adapter|a.get_info().device_type==wgpu::DeviceType::Cpu;
        let mut compatible=adapters.iter().filter(usable);
        let selected=if software_only {compatible.find(software)} else {
            compatible.clone().find(|a|!software(a)).or_else(||compatible.find(software))
        };
        if let Some(adapter)=selected {
            if diagnostics {
                let info=adapter.get_info();
                eprintln!("DrakOps graphics: {:?} / {:?} / {}",info.backend,info.device_type,info.name);
            }
            return Ok(adapter.clone());
        }
        Err(if software_only {"Windows did not provide a usable Direct3D 12 software adapter."}
            else {"Windows did not provide a usable Direct3D 12 graphics or software adapter."}.into())
    }));
    options.renderer=eframe::Renderer::Wgpu;
    options.wgpu_options.wgpu_setup=setup.into();
}

fn main()->eframe::Result {
    let demo=std::env::args().any(|arg|arg=="--demo");
    let demo_terminal=demo && std::env::args().any(|arg|arg=="--demo-terminal");
    let mut viewport=egui::ViewportBuilder::default().with_inner_size([1100.,720.]).with_min_inner_size([675.,460.]);
    if let Ok(icon)=eframe::icon_data::from_png_bytes(include_bytes!("../assets/brand/drakops.png")){
        viewport=viewport.with_icon(icon);
    }
    let options=eframe::NativeOptions{viewport,..Default::default()};
    #[cfg(windows)]
    let options={
        let mut options=options;
        let software_only=std::env::args().any(|a|a=="--software-renderer");
        let diagnostics=software_only || std::env::args().any(|a|a=="--graphics-diagnostics");
        windows_graphics(&mut options,software_only,diagnostics);
        options
    };
    eframe::run_native(TITLE,options,Box::new(move |cc|{let mut app=App::new(cc,demo); if demo_terminal {let device=app.book.devices[0].clone();app.terminal.start(&device,Route::Saved,true);} Ok(Box::new(app))}))
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test] fn brand_icon_decodes_at_expected_size(){
        let icon=eframe::icon_data::from_png_bytes(include_bytes!("../assets/brand/drakops.png")).unwrap();
        assert_eq!((icon.width,icon.height),(256,256));
        assert_eq!(icon.rgba[3],0);
        assert!(TITLE.contains("DrakOps"));
    }

    #[test] fn older_inventory_preserves_devices_without_kind() {
        let raw=r#"{"alias":"test","name":"Example","host":"192.0.2.8","lan":"","tailscale":"","user":"alex","port":22,"tags":"","notes":""}"#;
        let mut d:Device=serde_json::from_str(raw).unwrap();
        assert_eq!(d.kind,DeviceKind::Unspecified);
        assert!(!serde_json::to_string(&d).unwrap().contains("kind"));
        d.kind=DeviceKind::Laptop;
        assert_eq!(serde_json::from_str::<Device>(&serde_json::to_string(&d).unwrap()).unwrap(),d);
    }

    #[test] fn removal_and_undo_keep_neighbors_and_never_replace_changed_or_reused_alias(){
        let mut book=demo_book();
        let original=book.devices.clone();
        let old=original[1].clone();
        let mut edited=old.clone();edited.name="Different entry".into();
        assert!(book.remove_reviewed(&edited).is_err());
        assert_eq!(book.devices,original);
        let (index,deleted)=book.remove_reviewed(&old).unwrap();
        assert_eq!(index,1);
        assert_eq!(book.devices.iter().map(|d|d.alias.as_str()).collect::<Vec<_>>(),["fedora","laptop","sensor"]);
        book.restore_removed(index,deleted.clone()).unwrap();
        assert_eq!(book.devices,original);
        assert!(book.restore_removed(index,deleted).is_err());
        assert_eq!(book.devices,original);
    }
    #[test] fn theme_inventory_is_complete_and_resolves_every_id(){
        let ts=theme::inventory();
        assert_eq!(ts.len(),28);
        assert_eq!(ts.iter().filter(|t|t.id.starts_with("dlms-")).count(),5);
        assert_eq!(ts.iter().filter(|t|t.id.starts_with("omarchy-")).count(),22);
        assert_eq!(ts.iter().map(|t|&t.id).collect::<std::collections::BTreeSet<_>>().len(),28);
        let ctx=egui::Context::default();
        for t in ts{assert_eq!(&theme::find(&t.id).id,&t.id);theme::apply(&ctx,&t.id);assert!(theme::contrast(t.roles.text,t.roles.background)>=4.5);}
    }
}


#[cfg(test)]
#[test]
fn csv_choice_is_path_only_and_cancel_keeps_current() {
    let mut path=String::from("existing.csv");
    assert!(!select_csv_path(&mut path,None));
    assert_eq!(path,"existing.csv");
    assert!(!select_csv_path(&mut path,Some(PathBuf::from("existing.csv"))));
    assert!(select_csv_path(&mut path,Some(PathBuf::from("fixture.csv"))));
    assert_eq!(path,"fixture.csv");
    let rows=book::parse_csv(b"alias,hostname,user,port\nlocal,192.0.2.55,alex,2202\nbackup,backup.example,lee,22\n").unwrap();
    assert_eq!(rows.len(),2);
    assert_eq!((&*rows[0].alias,&*rows[0].host,&*rows[0].user,rows[0].port),("local","192.0.2.55","alex",2202));
    assert_eq!((&*rows[1].alias,rows[1].port),("backup",22));

}


#[cfg(test)]
#[test]
fn explicit_route_does_not_follow_other_devices() {
    let mut routes=BTreeMap::new();
    let dev=Device{alias:"dev".into(),host:"dev.example".into(),lan:"192.0.2.3".into(),user:"tester".into(),..Device::default()};
    let other=Device{alias:"elsewhere".into(),host:"other.example".into(),user:"tester".into(),..Device::default()};
    assert_eq!(connection_routes(&dev),vec![Route::Saved,Route::Lan]);
    assert_eq!(connection_routes(&other),vec![Route::Saved]);
    assert_eq!(connection_route(&routes,&dev),Route::Saved);
    routes.insert(dev.alias.clone(),(Route::Lan,dev.lan.clone()));
    assert_eq!(connection_route(&routes,&other),Route::Saved);
    assert_eq!(connection_route(&routes,&dev),Route::Lan);
    let changed=Device{lan:"192.0.2.4".into(),..dev};
    assert_eq!(connection_route(&routes,&changed),Route::Saved);
}
