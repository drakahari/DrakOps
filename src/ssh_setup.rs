//! First-use SSH setup for a saved Linux/OpenSSH account.
use crate::{book::{Device, Route}, external_terminal, egui};
use std::{collections::HashSet,fs::{self,File}, io::{Read,Write}, path::{Path,PathBuf}, process::{Command,Stdio}, sync::mpsc, time::Duration};

const MAX_PUBLIC_BYTES:u64=16*1024;

pub(super) fn verification_guidance(ui:&mut egui::Ui){
    ui.label("Already verified? Use Connect.");
    ui.label("First connection? Open the external terminal and compare the fingerprint it shows with one from the server console or your administrator. Accept only if they match.");
    ui.label("If the terminal asks for your account password after that, type it in that same terminal. The characters are hidden.");
    ui.label("If the terminal says the server identity changed, stop. Ask your administrator or check the server console before proceeding.");
}

fn public_path(raw:&str)->Result<(PathBuf,String),String>{
    let p=PathBuf::from(raw.trim());
    if !p.is_absolute() || p.extension().and_then(|x|x.to_str())!=Some("pub") {
        return Err("Choose an absolute path to an existing .pub file. Never choose a private key.".into());
    }
    #[cfg(windows)]
    if !matches!(p.components().next(),Some(std::path::Component::Prefix(x)) if matches!(x.kind(),std::path::Prefix::Disk(_)|std::path::Prefix::VerbatimDisk(_))){
        return Err("Choose a public key on a local Windows drive, not a network or device path.".into());
    }
    let meta=fs::symlink_metadata(&p).map_err(|_|"The public key file could not be opened.")?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len()>MAX_PUBLIC_BYTES{
        return Err("Use a regular, small OpenSSH .pub file, not a symlink.".into());
    }
    let p=fs::canonicalize(&p).map_err(|_|"Could not resolve the public key file.")?;
    let mut key=String::new();
    File::open(&p).map_err(|_|"Could not open that public key.")?.take(MAX_PUBLIC_BYTES+1).read_to_string(&mut key)
        .map_err(|_|"The public key was not a readable text file.")?;
    if key.len() as u64>MAX_PUBLIC_BYTES || !valid_public(&key){return Err("Use a single regular OpenSSH public-key line (not a private key, certificate, or authorized_keys list).".into());}
    Ok((p,key))
}

fn valid_public(key:&str)->bool{
    let mut lines=key.trim().lines();
    let Some(line)=lines.next() else {return false};
    if lines.next().is_some() || line.chars().any(|c|c.is_control() && c!='\t'){return false;}
    let mut tokens=line.split_ascii_whitespace();
    let (Some(kind),Some(b64))=(tokens.next(),tokens.next()) else {return false};
    if !matches!(kind,"ssh-ed25519"|"ssh-rsa"|"ecdsa-sha2-nistp256"|"ecdsa-sha2-nistp384"|"ecdsa-sha2-nistp521"){return false;}
    let decode=|b:u8|match b{b'A'..=b'Z'=>Some(b-b'A'),b'a'..=b'z'=>Some(b-b'a'+26),b'0'..=b'9'=>Some(b-b'0'+52),b'+'=>Some(62),b'/'=>Some(63),_=>None};
    let body=b64.trim_end_matches('=');
    if b64.len()<32 || b64.len()-body.len()>2 || body.len()%4==1 || !body.bytes().all(|b|decode(b).is_some()){return false;}
    // The SSH binary blob starts with the exact algorithm string. OpenSSH
    // performs full cryptographic parsing when the key is used.
    let mut prefix=Vec::new();
    let (mut acc,mut bits)=(0u32,0u32);
    for b in body.bytes().take(80){
        acc=(acc<<6)|u32::from(decode(b).unwrap());bits+=6;
        if bits>=8 {bits-=8;prefix.push((acc>>bits) as u8);}
    }
    prefix.get(..4).map(|b|u32::from_be_bytes(b.try_into().unwrap()) as usize)==Some(kind.len())
        && prefix.get(4..4+kind.len())==Some(kind.as_bytes())
}

fn destination(device:&Device,route:Route)->Result<(&str,&str),String>{
    device.ssh_args(route)?;
    if device.user.eq_ignore_ascii_case("root") && device.user!="root"{return Err("Use the exact account name root (lowercase).".into());}
    Ok((&device.user,device.address(route)))
}

// Distro ssh-copy-id implementations build a shell snippet with the key path.
// Keep a strict allowlist even though our own launcher quotes its arguments.
fn root_account(device:&Device)->bool { device.user=="root" }

fn installation_confirmed(device:&Device, reviewed:bool, root_reviewed:bool)->bool {
    reviewed && (!root_account(device) || root_reviewed)
}

fn standard_install_confirmed(device:&Device,reviewed:bool,root_reviewed:bool,standard_reviewed:bool)->bool{
    standard_reviewed && installation_confirmed(device,reviewed,root_reviewed)
}

struct ReviewedKey {
    source: PathBuf,
    line: String,
    fingerprint: String,
}

fn normalize_public(key:&str)->Result<String,String>{
    if !valid_public(key){return Err("Choose a single OpenSSH public key.".into());}
    let mut fields=key.split_ascii_whitespace();
    Ok(format!("{} {}",fields.next().unwrap(),fields.next().unwrap()))
}

fn fingerprint(line:&str)->Result<String,String>{
    let ssh=external_terminal::system_ssh()?;
    let leaf=if cfg!(windows) {"ssh-keygen.exe"} else {"ssh-keygen"};
    let tool=ssh.with_file_name(leaf);
    if !tool.is_file(){return Err("System ssh-keygen is unavailable. Review and install the public key manually outside DrakOps.".into());}
    let dir=tempfile::Builder::new().prefix("drakops-key-review-").tempdir()
        .map_err(|_|"Could not prepare local key review.")?;
    let snapshot=dir.path().join("reviewed.pub");
    let mut options=fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file=options.open(&snapshot).map_err(|_|"Could not prepare local key review.")?;
    writeln!(file,"{line}").map_err(|_|"Could not write local key review.")?;
    drop(file);
    let mut cmd=Command::new(tool);
    cmd.args(["-l","-E","sha256","-f"]).arg(&snapshot).stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let output=cmd.output().map_err(|_|"Could not run local ssh-keygen.")?;
    if !output.status.success(){return Err("System ssh-keygen could not validate this public key.".into());}
    let out=std::str::from_utf8(&output.stdout).map_err(|_|"Could not read local key fingerprint.")?;
    single_fingerprint(out).ok_or_else(||"No valid local SHA-256 fingerprint was returned.".into())
}

fn single_fingerprint(out:&str)->Option<String>{
    let mut lines=out.lines();
    let line=lines.next()?;
    if lines.next().is_some(){return None;}
    let mut fields=line.split_ascii_whitespace();
    fields.next()?.parse::<u32>().ok()?;
    let printable=fields.next()?;
    let digest=printable.strip_prefix("SHA256:")?;
    (digest.len()==43 && digest.bytes().all(|b|b.is_ascii_alphanumeric() || b"+/".contains(&b)))
        .then(||printable.to_owned())
}

impl ReviewedKey {
    fn load(raw:&str)->Result<Self,String>{
        let (source,key)=public_path(raw)?;
        let line=normalize_public(&key)?;
        let fingerprint=fingerprint(&line)?;
        Ok(Self{source,line,fingerprint})
    }

    fn approved_line(&self,device:&Device,consent:bool,root_consent:bool)->Option<String>{
        installation_confirmed(device,consent,root_consent).then(||self.line.clone())
    }
}

#[derive(Default)]
struct LaunchFeedback {
    message: String,
    watch: Option<external_terminal::LaunchWatch>,
}
impl LaunchFeedback {
    fn start(&mut self,result:Result<external_terminal::LaunchWatch,String>, pending:&str){
        match result {
            Ok(watch)=>{self.watch=Some(watch);self.message=pending.into();}
            Err(e)=>{self.watch=None;self.message=e;}
        }
    }
    fn poll(&mut self,ctx:&egui::Context, done:&str){
        let Some(watch)=&self.watch else{return;};
        match watch.try_recv(){
            Ok(Ok(()))=>{self.watch=None;self.message=done.into();}
            Ok(Err(e))=>{self.watch=None;self.message=e;}
            Err(mpsc::TryRecvError::Empty)=>ctx.request_repaint_after(Duration::from_millis(150)),
            Err(_)=>{self.watch=None;self.message="Terminal launch status was unavailable. Check your terminal.".into();}
        }
    }
}

fn install_blocker(valid_key:bool,reviewed:bool,root:bool,root_reviewed:bool) -> Option<&'static str> {
    if !valid_key {return Some("Choose a valid .pub file above first.");}
    if !reviewed {return Some("Check the account and public-key review box.");}
    if root && !root_reviewed {return Some("Acknowledge that this root key grants full administrative access.");}
    None
}

fn start_key_picker(start_from:Option<PathBuf>)->Result<mpsc::Receiver<Option<PathBuf>>,String>{
    let (tx,rx)=mpsc::channel();
    std::thread::Builder::new().name("homeops-public-key-picker".into()).spawn(move||{
        let mut dialog=rfd::FileDialog::new().set_title("Choose an SSH public key (.pub)").add_filter("SSH public key",&["pub"]);
        if let Some(root)=start_from{dialog=dialog.set_directory(root);}
        let _=tx.send(dialog.pick_file());
    }).map_err(|_|"Could not open the file chooser. Enter the full .pub path instead.".to_owned())?;
    Ok(rx)
}

fn default_key_directory(home:&Path)->PathBuf{
    let ssh=home.join(".ssh");
    if ssh.is_dir(){ssh}else{home.to_owned()}
}

fn wizard_window<'a>(ctx:&egui::Context,open:&'a mut bool)->egui::Window<'a>{
    let viewport=ctx.content_rect();
    let max=(viewport.size()-egui::vec2(52.,84.)).max(egui::vec2(50.,48.));
    let frame=egui::Frame::window(&ctx.style()).inner_margin(egui::Margin::same(20));
    egui::Window::new("Set up key login").open(open).frame(frame).constrain_to(viewport)
        .default_size(max.min(egui::vec2(596.,480.)))
        .min_size(max.min(egui::vec2(260.,180.))).max_size(max)
        // The window's own scroll area uses the live resized height.
        .resizable([true,true]).vscroll(true)
}

fn wizard_style(ui:&mut egui::Ui){
    let style=ui.style_mut();
    for (role,size,family) in [
        (egui::TextStyle::Heading,25.,egui::FontFamily::Proportional),
        (egui::TextStyle::Body,16.5,egui::FontFamily::Proportional),
        (egui::TextStyle::Button,16.5,egui::FontFamily::Proportional),
        (egui::TextStyle::Small,14.5,egui::FontFamily::Proportional),
        (egui::TextStyle::Monospace,15.5,egui::FontFamily::Monospace),
    ] {style.text_styles.insert(role,egui::FontId::new(size,family));}
    style.spacing.item_spacing=egui::vec2(10.,10.);
    style.spacing.interact_size.y=34.;
    style.spacing.button_padding=egui::vec2(12.,6.);
}

fn wizard_action(ui:&mut egui::Ui,label:&str,enabled:bool)->egui::Response{
    let width=ui.available_width().max(1.);
    crate::theme::primary_button(ui,label,enabled,egui::vec2(width,46.))
}

fn wizard_secondary(ui:&mut egui::Ui,text:&str){
    ui.label(egui::RichText::new(text).small().weak());
}

fn recommended_key(home:&Path)->Option<PathBuf>{
    ["id_ed25519.pub","id_rsa.pub","id_ecdsa.pub"].into_iter()
        .map(|leaf|home.join(".ssh").join(leaf))
        .find(|p|fs::symlink_metadata(p).is_ok_and(|m|m.is_file() && m.len()<=MAX_PUBLIC_BYTES))
}

#[derive(Default)]
pub(super) struct InstallAttempts {
    unix_requested: HashSet<InstallIdentity>,
    windows_id: Option<InstallIdentity>,
    windows: LaunchFeedback,
}
impl InstallAttempts {
    fn windows_pending(&self)->bool { self.windows.watch.is_some() }
    fn poll_windows(&mut self,ctx:&egui::Context) {
        self.windows.poll(ctx,"Setup terminal closed. Review its result before assuming the key works; start a fresh connection to test again.");
    }
}

#[derive(PartialEq,Eq,Hash)]
struct InstallIdentity {host:String,port:u16,user:String,fingerprint:String}

impl InstallIdentity {
    fn new(device:&Device,route:Route,key:&ReviewedKey)->Self {
        let address=device.address(route);
        let host=address.parse::<std::net::IpAddr>().map(|ip|ip.to_string()).unwrap_or_else(|_|address.to_ascii_lowercase());
        Self{host,port:device.port,user:device.user.clone(),fingerprint:key.fingerprint.clone()}
    }
}

pub(super) struct Setup {
    device:Device, route:Route, key:String, consent:bool, root_consent:bool, standard_install_consent:bool,
    status:String,
    reviewing:bool,
    trust: LaunchFeedback,
    install: LaunchFeedback,
    key_check: Option<Result<ReviewedKey,String>>,
    picker: Option<mpsc::Receiver<Option<PathBuf>>>,
}
impl Setup {
    pub(super) fn new(device:Device,route:Route)->Self{
        Self{device,route,key:String::new(),consent:false,root_consent:false,standard_install_consent:false,status:String::new(),reviewing:false,trust:Default::default(),install:Default::default(),key_check:None,picker:None}
    }
    pub(super) fn refers_to(&self,d:&Device)->bool{self.device.alias.eq_ignore_ascii_case(&d.alias)}
    fn reset_approval(&mut self){
        self.consent=false;
        self.root_consent=false;
        self.standard_install_consent=false;
        self.status.clear();
    }
    fn check_key(&mut self){
        self.reset_approval();
        self.key_check=(!self.key.trim().is_empty()).then(||ReviewedKey::load(&self.key));
    }
    fn choose_key(&mut self,selected:Option<PathBuf>){
        let Some(path)=selected else{return;}; // Cancel leaves the current selection and consent untouched.
        let Some(path)=path.to_str() else {
            self.key.clear();
            self.check_key();
            self.key_check=Some(Err("This filename cannot be represented. Select a .pub file with a supported name.".into()));
            return;
        };
        self.key=path.to_owned();self.check_key();
    }

    fn review_selected(&mut self){
        if matches!(self.key_check,Some(Ok(_))){
            self.reset_approval();
            self.reviewing=true;
        }
    }

    fn open_picker(&mut self,start:Option<PathBuf>){
        match start_key_picker(start) {
            Ok(rx)=>self.picker=Some(rx),
            Err(e)=>self.status=e,
        }
    }

    fn manual_controls(&mut self,ui:&mut egui::Ui,enabled:bool,primary:bool){
        ui.label("Add this public key to the server account’s authorized_keys over a trusted connection, or ask its administrator. Keep existing entries.");
        let clicked=if primary {
            wizard_action(ui,"Copy reviewed public key",enabled).clicked()
        } else {ui.add_enabled(enabled,egui::Button::new("Copy reviewed public key")).clicked()};
        if clicked && let Some(Ok(key))=&self.key_check
            && let Some(line)=key.approved_line(&self.device,self.consent,self.root_consent)
        {
            ui.ctx().copy_text(line);
            self.status="Copy requested. Compare the pasted key with this fingerprint. Nothing has been installed.".into();
        }
    }

    fn other_ways(&mut self,ui:&mut egui::Ui,enabled:bool,manual:bool,allow_copy:bool){
        ui.collapsing("Other ways to set up", |ui| {
            ui.strong("Verify this server");
            verification_guidance(ui);
            if ui.add_enabled(enabled && self.trust.watch.is_none(),egui::Button::new("Open in external terminal")).clicked(){
                self.trust.start(external_terminal::open_tracked(&self.device,self.route),"External terminal requested. Check the server fingerprint there.");
            }
            if !self.trust.message.is_empty(){ui.weak(&self.trust.message);}
            if manual {ui.separator();self.manual_controls(ui,allow_copy,false);}
            ui.weak("Already able to connect? Skip setup. Tailscale SSH doesn’t use this key; regular OpenSSH over a Tailscale address may use it.");
            ui.weak("A successful login doesn’t show which key was used. Check in your external OpenSSH terminal if needed.");
        });
    }

    pub(super) fn show(mut self,ctx:&egui::Context,attempts:&mut InstallAttempts,terminal:&mut crate::terminal_ui::TerminalUi,connection_allowed:bool)->Option<Self>{
        if let Some(picker)=&self.picker {
            match picker.try_recv(){
                Ok(choice)=>{self.picker=None;if choice.is_some(){self.choose_key(choice);self.review_selected();}}
                Err(mpsc::TryRecvError::Empty)=>ctx.request_repaint_after(Duration::from_millis(100)),
                Err(_)=>{self.picker=None;self.status="File chooser unavailable. Enter the full .pub path instead.".into();}
            }
        }
        self.trust.poll(ctx,"Terminal launcher finished. Continue only after verifying the host fingerprint.");
        if cfg!(windows) {
            attempts.poll_windows(ctx);
        } else {
            self.install.poll(ctx,"Follow the external terminal prompts. If it says a key was added, close it and start a new connection.");
        }
        let mut open=true;
        let mut open_new_tab=false;
        wizard_window(ctx,&mut open).show(ctx,|ui|{
            wizard_style(ui);
                let name=if self.device.name.is_empty(){&self.device.alias}else{&self.device.name};
                wizard_secondary(ui,if self.reviewing{"STEP 2 OF 2"}else{"STEP 1 OF 2"});
                ui.heading(if self.reviewing{"Review and install"}else{"Choose your SSH key"});
                wizard_secondary(ui,&format!("{name} · {}",self.device.user));
                let invalid=destination(&self.device,self.route).err();
                if let Some(e)=&invalid{ui.colored_label(crate::theme::roles(ctx).error,e);}
                let enabled=invalid.is_none();
                let can_edit=enabled && self.picker.is_none() && self.install.watch.is_none() && !attempts.windows_pending();
                ui.separator();

                if !self.reviewing {
                    ui.label("Only needed if this computer asks for a server password.");
                    let home=directories::BaseDirs::new();
                    let start=home.as_ref().map(|d|default_key_directory(d.home_dir()));
                    let suggested=home.as_ref().and_then(|d|recommended_key(d.home_dir()));
                    if let Some(path)=suggested {
                        let label=path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
                        if wizard_action(ui,"Use my SSH key",can_edit).clicked(){
                            self.choose_key(Some(path.clone()));
                            self.review_selected();
                        }
                        ui.weak(label);
                    } else {
                        if wizard_action(ui,"Choose SSH key",can_edit).clicked(){
                            self.open_picker(start.clone());
                        }
                        ui.weak("Choose a public key file ending in .pub.");
                    }
                    ui.collapsing("Choose a different key", |ui|{
                        if ui.add_enabled(can_edit,egui::Button::new("Browse for a .pub file")).clicked(){self.open_picker(start.clone());}
                        ui.add_enabled(can_edit,egui::TextEdit::singleline(&mut self.key).hint_text("Full path to a .pub file").desired_width(f32::INFINITY));
                        if ui.add_enabled(can_edit && !self.key.trim().is_empty(),egui::Button::new("Review this key")).clicked(){
                            self.check_key();
                            self.review_selected();
                        }
                    });
                    if self.picker.is_some(){ui.spinner();}
                    if let Some(Err(e))=&self.key_check{ui.colored_label(crate::theme::roles(ctx).error,e);}
                    self.other_ways(ui,enabled,false,false);
                } else {
                    let roles=crate::theme::roles(ctx);
                    egui::Frame::new().fill(roles.background).stroke(egui::Stroke::new(1_f32,roles.border))
                        .corner_radius(10).inner_margin(egui::Margin::same(14)).show(ui,|ui|{
                            ui.set_min_width(ui.available_width().max(1.));
                            ui.spacing_mut().item_spacing.y=5.;
                            ui.strong("Destination");
                            ui.label(crate::endpoint(&self.device,self.route));
                            wizard_secondary(ui,&format!("{} route",self.route.label()));
                            if let Some(Ok(key))=&self.key_check {
                                ui.strong("Public key");
                                let path=&key.source;
                                wizard_secondary(ui,&path.file_name().unwrap_or(path.as_os_str()).to_string_lossy());
                                ui.monospace(&key.fingerprint);
                            }
                        });
                    ui.add_space(2.);
                    ui.strong("Before installing");
                    ui.checkbox(&mut self.consent,"This is the right account and key, and I administer this computer.");
                    if root_account(&self.device){
                        ui.checkbox(&mut self.root_consent,"I approve full administrator access for this key (root).");
                    }
                    let valid_key=matches!(self.key_check,Some(Ok(_)));
                    let ready=can_edit && install_blocker(valid_key,self.consent,root_account(&self.device),self.root_consent).is_none();
                    let automatic=external_terminal::guided_install_available();
                    if automatic {
                        let key=self.key_check.as_ref().and_then(|r|r.as_ref().ok());
                        let identity=key.map(|k|InstallIdentity::new(&self.device,self.route,k));
                        let already=if cfg!(windows) {attempts.windows_pending()} else {
                            identity.as_ref().is_some_and(|id|attempts.unix_requested.contains(id))
                        };
                        ui.add_enabled(can_edit && valid_key && !already,egui::Checkbox::new(&mut self.standard_install_consent,
                            if cfg!(windows) {"This is a Linux or Unix account using standard OpenSSH key setup."}
                            else {"I checked: this key is not already installed, and I want standard SSH access."}));
                        wizard_secondary(ui,"Need restrictions or not sure? Use the manual steps below.");
                        if cfg!(windows) {
                            wizard_secondary(ui,"Only the reviewed public key is sent. If asked, enter your key passphrase or server password in the trusted terminal. Read the result in that window.");
                            wizard_secondary(ui,"Use manual setup if this connection needs a jump host, SSH alias settings, or custom server key storage.");
                        }
                        let approved=standard_install_confirmed(&self.device,self.consent,self.root_consent,self.standard_install_consent);
                        if wizard_action(ui,"Install key in trusted terminal",ready && !already && approved).clicked()
                            && let Some(key)=key
                            && let Some(line)=key.approved_line(&self.device,self.consent,self.root_consent)
                            && let Some(id)=identity
                        {
                            if cfg!(windows) {
                                let result=external_terminal::open_public_install(self.device.clone(),self.route,line,&key.source,&key.fingerprint);
                                attempts.windows_id=Some(id);
                                attempts.windows.start(result,"Setup terminal opened. Read the result there, then close it to retry or continue.");
                            } else if attempts.unix_requested.insert(id) {
                                let result=external_terminal::open_public_install(self.device.clone(),self.route,line,&key.source,&key.fingerprint);
                                self.install.start(result,"Opening an external terminal. Enter any password there, not in DrakOps.");
                            }
                        }
                        if already {
                            ui.weak(if cfg!(windows){"A setup terminal is open. Close it before starting another."}
                                else{"Already requested for this key and account. Check the external terminal for the result."});
                        }
                        let feedback=if cfg!(windows) {
                            attempts.windows_id.as_ref().filter(|id|key.is_some_and(|k|**id==InstallIdentity::new(&self.device,self.route,k)))
                                .map(|_|&attempts.windows)
                        } else {Some(&self.install)};
                        if let Some(feedback)=feedback{
                            if feedback.watch.is_some(){ui.spinner();}
                            if !feedback.message.is_empty(){ui.label(&feedback.message);}
                        }
                    } else {
                        self.manual_controls(ui,ready,true);
                    }
                    ui.separator();
                    ui.strong("Finish setup");
                    if cfg!(windows) {
                        wizard_secondary(ui,"Read the result in the separate terminal. Only a result that says key login worked or was verified confirms success. Closing that window does not confirm success.");
                    } else {
                        wizard_secondary(ui,"Read the result in the separate terminal. If it reports that the key was added, try a new connection to check that login works.");
                    }
                    if wizard_action(ui,"Open new SSH tab",can_edit && connection_allowed && terminal.has_tab_space()).clicked(){
                        open_new_tab=true;
                    }
                    if !terminal.has_tab_space(){ui.weak("Close a tab to open a new one.");}
                    wizard_secondary(ui,"This starts a fresh connection without reusing an open tab.");
                    ui.separator();
                    self.other_ways(ui,enabled,automatic,ready);
                    if ui.add_enabled(can_edit,egui::Button::new("Back")).clicked(){
                        self.reviewing=false;
                        self.reset_approval();
                    }
                }
                if !self.status.is_empty(){ui.label(&self.status);}
        });
        if open_new_tab{
            terminal.start_new_tab(&self.device,self.route);
            return None;
        }
        open.then_some(self)
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn choosing_checks_a_public_file_and_resets_preexisting_consent(){
        let mut setup=Setup::new(target(),Route::Lan);
        assert_eq!(install_blocker(false,true,true,true),Some("Choose a valid .pub file above first."));
        assert!(install_blocker(true,true,true,false).is_some());
        assert!(install_blocker(true,true,false,false).is_none());
        let tmp=tempfile::tempdir().unwrap();
        let p=tmp.path().join("fixture.pub");
        fs::write(&p,"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA fixture").unwrap();
        setup.consent=true; setup.root_consent=true; setup.standard_install_consent=true;
        setup.choose_key(Some(p.clone()));
        assert!(setup.key_check.is_some());
        assert!(!setup.consent && !setup.root_consent && !setup.standard_install_consent);
        setup.consent=true;
        setup.choose_key(None);
        assert!(setup.consent);
        assert_eq!(setup.key,p.display().to_string());
        let private=tmp.path().join("fixture");
        setup.choose_key(Some(private));
        assert!(matches!(setup.key_check,Some(Err(_))));
        assert!(!setup.consent);
        assert_eq!(default_key_directory(tmp.path()),tmp.path());
        fs::create_dir(tmp.path().join(".ssh")).unwrap();
        assert_eq!(default_key_directory(tmp.path()),tmp.path().join(".ssh"));
        let ctx=egui::Context::default();
        let (tx,rx)=mpsc::channel();
        let mut feedback=LaunchFeedback::default();
        feedback.start(Ok(rx),"Requested");
        tx.send(Err("Synthetic terminal startup failure".into())).unwrap();
        feedback.poll(&ctx,"Launcher exited");
        assert_eq!(feedback.message,"Synthetic terminal startup failure");
    }

    fn target()->Device{Device{alias:"lab".into(),lan:"192.0.2.8".into(),user:"fixture".into(),port:2202,..Default::default()}}

    #[test]
    fn reviewed_snapshot_is_exact_and_root_requires_separate_consent(){
        let tmp=tempfile::tempdir().unwrap();
        let source=tmp.path().join("fixture.pub");
        let input="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA ignored";
        let reviewed=ReviewedKey{source:source.clone(),line:normalize_public(input).unwrap(),fingerprint:"fixture".into()};
        fs::write(source,"different file contents").unwrap();
        assert_eq!(reviewed.approved_line(&target(),true,false),Some(input.strip_suffix(" ignored").unwrap().to_owned()));
        let root=Device{user:"root".into(),..target()};
        assert!(reviewed.approved_line(&root,true,false).is_none());
        assert!(reviewed.approved_line(&root,false,true).is_none());
        assert!(reviewed.approved_line(&root,true,true).is_some());
    }

    #[test]
    fn wizard_window_grows_in_both_dimensions_and_reveals_more_content(){
        let ctx=egui::Context::default();
        let mut open=true;
        let mut frame=|events|{
            let mut inner=egui::Rect::NOTHING;
            let mut outer=egui::Rect::NOTHING;
            let input=egui::RawInput{
                screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(1024.,740.))),
                events,..Default::default()
            };
            let _=ctx.run(input,|ctx|{
                outer=wizard_window(ctx,&mut open).show(ctx,|ui|{
                    inner=ui.clip_rect();
                    for n in 0..55 {ui.label(format!("Wizard fixture row {n}"));}
                }).unwrap().response.rect;
            });
            (outer,inner)
        };
        frame(vec![]);
        let (outer,inner)=frame(vec![]);
        let corner=outer.right_bottom()-egui::vec2(2.,2.);
        let end=corner+egui::vec2(72.,72.);
        let mouse=|pos,pressed|egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed,modifiers:egui::Modifiers::NONE};
        frame(vec![egui::Event::PointerMoved(corner),mouse(corner,true)]);
        frame(vec![egui::Event::PointerMoved(end)]);
        let (_,after)=frame(vec![mouse(end,false)]);
        assert!(after.width()>inner.width()+20., "{inner:?} -> {after:?}");
        assert!(after.height()>inner.height()+20., "{inner:?} -> {after:?}");
    }

    #[test]
    fn recommends_ed25519_and_review_starts_with_fresh_approval(){
        let tmp=tempfile::tempdir().unwrap();
        let ssh=tmp.path().join(".ssh");
        assert!(recommended_key(tmp.path()).is_none());
        fs::create_dir(&ssh).unwrap();
        fs::write(ssh.join("id_rsa.pub"),"synthetic").unwrap();
        assert_eq!(recommended_key(tmp.path()).unwrap(),ssh.join("id_rsa.pub"));
        fs::write(ssh.join("id_ed25519.pub"),"synthetic").unwrap();
        assert_eq!(recommended_key(tmp.path()).unwrap(),ssh.join("id_ed25519.pub"));
        let mut setup=Setup::new(target(),Route::Lan);
        setup.review_selected();
        assert!(!setup.reviewing);
        setup.key_check=Some(Ok(ReviewedKey{source:ssh.join("id_ed25519.pub"),line:"fixture".into(),fingerprint:"fixture".into()}));
        setup.consent=true;setup.root_consent=true;setup.standard_install_consent=true;
        setup.review_selected();
        assert!(setup.reviewing && !setup.consent && !setup.root_consent && !setup.standard_install_consent);
    }

    #[test]
    fn standard_install_requires_separate_opt_in_but_manual_review_does_not(){
        let normal=target();
        assert!(installation_confirmed(&normal,true,false));
        assert!(!standard_install_confirmed(&normal,true,false,false));
        assert!(!standard_install_confirmed(&normal,false,false,true));
        assert!(standard_install_confirmed(&normal,true,false,true));
        let root=Device{user:"root".into(),..normal};
        assert!(!standard_install_confirmed(&root,true,false,true));
        assert!(!standard_install_confirmed(&root,true,true,false));
        assert!(standard_install_confirmed(&root,true,true,true));
    }

    #[test]
    fn attempts_are_once_per_reviewed_fingerprint_and_endpoint_during_app_session(){
        let mut attempts=InstallAttempts::default();
        let d=target();
        let mut key=ReviewedKey{source:"fixture.pub".into(),line:"public fixture".into(),fingerprint:"one".into()};
        assert!(attempts.unix_requested.insert(InstallIdentity::new(&d,Route::Lan,&key)));
        assert!(!attempts.unix_requested.insert(InstallIdentity::new(&d,Route::Lan,&key)));
        assert!(attempts.unix_requested.insert(InstallIdentity::new(&Device{port:22,..d.clone()},Route::Lan,&key)));
        key.fingerprint="two".into();
        assert!(attempts.unix_requested.insert(InstallIdentity::new(&d,Route::Lan,&key)));
    }

    #[test]
    fn windows_installer_can_retry_after_terminal_closes_or_fails() {
        let ctx=egui::Context::default();
        let mut attempts=InstallAttempts::default();
        let (tx,rx)=mpsc::channel();
        attempts.windows.start(Ok(rx),"Pending");
        attempts.poll_windows(&ctx);
        assert!(attempts.windows_pending());
        tx.send(Ok(())).unwrap();
        attempts.poll_windows(&ctx);
        assert!(!attempts.windows_pending());
        assert!(attempts.windows.message.contains("Review its result"));
        attempts.windows.start(Err("No launch".into()),"Pending");
        assert!(!attempts.windows_pending());
        assert_eq!(attempts.windows.message,"No launch");
    }

    #[test]
    fn local_fingerprint_parser_requires_single_sha256_fingerprint(){
        let hash="A".repeat(43);
        let line=format!("256 SHA256:{hash} reviewed (ED25519)");
        assert_eq!(single_fingerprint(&line),Some(format!("SHA256:{hash}")));
        assert_eq!(single_fingerprint("256 SHA256:bad (ED25519)"),None);
        assert_eq!(single_fingerprint(&format!("{line}\n{line}")),None);
    }

    #[test]
    fn accepts_plain_public_line_not_private_or_multiple_keys(){
        let line="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA fixture";
        assert!(valid_public(line));
        assert!(!valid_public(&format!("{line}\n{line}")));
        assert!(!valid_public("-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert!(!valid_public(&line.replace("ssh-ed25519 ","ssh-rsa ")));
    }
}
