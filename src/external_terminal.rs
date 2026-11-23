//! User-initiated recovery route. OpenSSH handles host verification and auth.
use crate::book::{Device, Route};
use std::{path::PathBuf, process::Command, sync::mpsc};
pub type LaunchWatch = mpsc::Receiver<Result<(),String>>;

pub fn external_args(device: &Device, route: Route) -> Result<Vec<String>, String> {
    // Ask is explicit so a permissive config cannot silently accept a new host here.
    // This still refuses a changed server key and leaves all other local config intact.
    let mut args=vec!["-oStrictHostKeyChecking=ask".into()];
    args.extend(device.ssh_args(route)?);
    Ok(args)
}

#[cfg(any(test,target_os="linux"))]
fn linux_args(launcher: &str, ssh: &str, args: &[String], hold: bool) -> Vec<String> {
    let option=if launcher.ends_with("/xdg-terminal-exec") {"--"}else{"-e"};
    let prefix=(hold && launcher.ends_with("/foot")).then(||"--hold".to_owned());
    prefix.into_iter().chain([option.to_owned(),ssh.to_owned()]).chain(args.iter().cloned()).collect()
}

#[cfg(any(test,unix))]
fn mac_command(ssh: &str, args: &[String]) -> String {
    // Terminal's do script API accepts one shell line, not argv. Quote each
    // independently; only validated Device::ssh_args inputs are used.
    let quote=|a:&str|format!("'{}'",a.replace('\'',"'\"'\"'"));
    std::iter::once(ssh).chain(args.iter().map(String::as_str)).map(quote).collect::<Vec<_>>().join(" ")
}

#[cfg(any(test,target_os="windows"))]
fn powershell_command(ssh: &str, args: &[String]) -> String {
    let quote=|a:&str|format!("'{}'",a.replace('\'',"''"));
    format!("& {}",std::iter::once(ssh).chain(args.iter().map(String::as_str)).map(quote).collect::<Vec<_>>().join(" "))
}

#[cfg(any(target_os="macos",target_os="linux"))]
fn system_posix_ssh() -> Option<PathBuf> {
    ["/usr/bin/ssh","/bin/ssh"].into_iter().map(PathBuf::from).find(|p|p.is_file())
}

pub fn system_ssh() -> Result<PathBuf,String> {
    #[cfg(target_os="windows")]
    {
        let root=std::env::var_os("SystemRoot").map(PathBuf::from).filter(|p|p.is_absolute())
            .ok_or("Could not locate Windows OpenSSH.")?;
        let exe=root.join("System32").join("OpenSSH").join("ssh.exe");
        if exe.is_file(){return Ok(exe);}
    }
    #[cfg(any(target_os="macos",target_os="linux"))]
    if let Some(exe)=system_posix_ssh(){return Ok(exe);}
    Err("System OpenSSH was not found. Use an ordinary terminal instead.".into())
}

#[cfg(not(target_os="windows"))]
pub(super) fn system_copy_id()->Option<&'static str>{
    ["/usr/bin/ssh-copy-id","/bin/ssh-copy-id"].into_iter().find(|p|cfg!(unix) && std::path::Path::new(p).is_file())
}

#[cfg(any(unix,test))]
fn public_install_args(device:&Device,route:Route,snapshot:&str)->Result<Vec<String>,String>{
    device.ssh_args(route)?;
    if !snapshot.starts_with("/tmp/drakops-key-") || !snapshot.ends_with("/selected.pub")
        || !snapshot.bytes().all(|b|b.is_ascii_alphanumeric() || b"/._-".contains(&b)) {
        return Err("Cannot safely stage the reviewed public key.".into());
    }
    Ok(vec!["-f".into(),"-i".into(),snapshot.into(),"-F".into(),"/dev/null".into(),
        "-oStrictHostKeyChecking=yes".into(),"-oUpdateHostKeys=no".into(),
        "-oControlMaster=no".into(),"-oControlPath=none".into(),"-oControlPersist=no".into(),
        "-oPermitLocalCommand=no".into(),"-oClearAllForwardings=yes".into(),
        "-oForwardAgent=no".into(),"-oForwardX11=no".into(),
        "-oConnectTimeout=10".into(),"-p".into(),device.port.to_string(),
        format!("{}@{}",device.user,device.address(route))])
}

#[cfg(any(unix,test))]
fn public_install_wrapper(tool:&str,args:&[String],expires:u64)->String{
    let invoke=mac_command(tool,args);
    format!("#!/bin/sh\nPATH=/usr/bin:/bin; export PATH\numask 077\ndir=${{0%/*}}\n        mkdir -- \"$dir/claimed\" || exit 1\n        cleanup() {{ rm -f -- \"$dir/selected.pub\" \"$0\"; rmdir -- \"$dir/claimed\" \"$dir\" 2>/dev/null || :; }}\n        trap cleanup 0\ntrap 'exit 130' INT\ntrap 'exit 143' TERM\ntrap 'exit 129' HUP\n        now=$(date +%s) || exit 1\n        if [ \"$now\" -gt {expires} ]; then printf '%s\\n' 'Setup request expired. No key was installed.'; exit 1; fi\n        {invoke}\n")
}

pub(super) fn guided_install_available()->bool{
    #[cfg(target_os="windows")]
    { system_ssh().is_ok_and(|ssh|ssh.with_file_name("ssh-keygen.exe").is_file()) && windows_powershell().is_ok() }
    #[cfg(not(target_os="windows"))]
    { system_copy_id().is_some() }
}

pub(super) fn open_public_install(device:Device,route:Route,line:String,_source:&std::path::Path,_fingerprint:&str)->Result<LaunchWatch,String>{
    #[cfg(unix)]
    {
        let tool=system_copy_id().ok_or("System ssh-copy-id is not available. Use the manual steps.")?;
        let (tx,rx)=mpsc::channel();
        std::thread::Builder::new().name("drakops-public-key-handoff".into()).spawn(move||{
            let _=tx.send(prepare_public_install(&device,route,&line,tool));
        }).map_err(|_|"Could not start the installer handoff.")?;
        Ok(rx)
    }
    #[cfg(target_os="windows")]
    {
        use std::os::windows::process::CommandExt;
        let ssh=system_ssh()?;
        let ps=windows_powershell()?;
        let install=crate::ssh_enroll::enroll_args(&device,route,&line)?;
        // Only OpenSSH loads the private identity. DrakOps checks its path
        // and supplies its already-reviewed public fingerprint for proof.
        let identity=_source.with_extension("");
        let path=identity.to_str().ok_or("Unsupported key path. Use the manual steps.")?;
        let path=path.strip_prefix(r"\\?\").unwrap_or(path);
        if !matches!(std::path::Path::new(path).components().next(),Some(std::path::Component::Prefix(prefix))
            if matches!(prefix.kind(),std::path::Prefix::Disk(_))) || path.contains('%')
            || !std::fs::symlink_metadata(&identity).is_ok_and(|m|m.is_file() && !m.file_type().is_symlink()){
            return Err("A matching private-key file wasn't found next to this .pub file on a local drive. Choose another .pub file or use the manual steps.".into());
        }
        let test=crate::ssh_enroll::login_args(&device,route,path)?;
        let keygen=ssh.with_file_name("ssh-keygen.exe");
        if !keygen.is_file(){return Err("System ssh-keygen is unavailable. Use manual setup.".into());}
        let verify_local=["-y".into(),"-f".into(),path.to_owned()];
        let script=crate::ssh_enroll::powershell_script(
            ssh.to_str().ok_or("Invalid OpenSSH path.")?,keygen.to_str().ok_or("Invalid ssh-keygen path.")?,
            &verify_local,&line,&install,&test,_fingerprint);
        let encoded=crate::ssh_enroll::encoded_powershell(&script);
        if encoded.len()>28_000{return Err("This key or path is too long for the setup terminal. Use the manual steps.".into());}
        let mut cmd=Command::new(ps);
        cmd.creation_flags(0x10).args(["-NoLogo","-NoProfile","-NoExit","-EncodedCommand"]).arg(encoded);
        launch(cmd)
    }
    #[cfg(not(any(unix,target_os="windows")))]
    {
        let _=(device,route,line);
        Err("Use the manual public-key steps on this computer.".into())
    }
}

#[cfg(unix)]
fn prepare_public_install(device:&Device,route:Route,line:&str,tool:&str)->Result<(),String>{
    use std::{fs,io::Write,os::unix::fs::{OpenOptionsExt,PermissionsExt},time::{Duration,Instant,SystemTime,UNIX_EPOCH}};
    let dir=tempfile::Builder::new().prefix("drakops-key-").permissions(fs::Permissions::from_mode(0o700)).tempdir_in("/tmp")
        .map_err(|_|"Could not stage the reviewed key privately.")?;
    let snapshot=dir.path().join("selected.pub");
    let script=dir.path().join("install.command");
    let args=public_install_args(device,route,snapshot.to_str().ok_or("Invalid temporary path.")?)?;
    let expires=SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_|"Invalid system time.")?.as_secs()+45;
    let put=|path:&std::path::Path,data:&[u8],mode:u32|{
        fs::OpenOptions::new().create_new(true).write(true).mode(mode).open(path)
            .and_then(|mut f|f.write_all(data)).map_err(|_|"Could not write the private installer files.")
    };
    put(&snapshot,format!("{line}\n").as_bytes(),0o400)?;
    put(&script,public_install_wrapper(tool,&args,expires).as_bytes(),0o700)?;
    let mut command=public_install_launcher(&script)?;
    let mut launcher=command.spawn().map_err(|_|"Could not start the installer terminal. Use the manual steps.")?;
    let claim=dir.path().join("claimed");
    let deadline=Instant::now()+Duration::from_secs(45);
    let reap=|mut launcher:std::process::Child|{
        let _=std::thread::Builder::new().name("drakops-reap-installer-launcher".into())
            .spawn(move||{let _=launcher.wait();});
    };
    loop {
        if claim.is_dir() || !dir.path().exists(){
            // Only the executing wrapper owns snapshot cleanup now. Launcher
            // exit is not installer completion (e.g. Terminal.app handoff).
            let _=dir.keep();
            reap(launcher);
            return Ok(());
        }
        let failed=launcher.try_wait().ok().flatten().is_some_and(|code|!code.success());
        if failed || Instant::now()>=deadline {
            // Race the wrapper on one atomic directory creation. If it won,
            // leave the live snapshot alone; it cleans up on actual exit.
            let outcome=match fs::create_dir(&claim) {
                Ok(())=>Err("Installer terminal did not start the handoff. Nothing was confirmed; use the manual steps.".into()),
                Err(e) if matches!(e.kind(),std::io::ErrorKind::AlreadyExists|std::io::ErrorKind::NotFound)=>{
                    let _=dir.keep();
                    Ok(())
                }
                Err(_)=>{
                    let _=dir.keep();
                    Err("Could not confirm the installer handoff. Use the manual steps.".into())
                }
            };
            reap(launcher);
            return outcome;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(target_os="linux")]
fn public_install_launcher(script:&std::path::Path)->Result<Command,String>{
    let launcher=["/usr/bin/foot","/usr/bin/xdg-terminal-exec","/usr/bin/kitty","/usr/bin/alacritty","/usr/bin/xterm"]
        .into_iter().find(|p|std::path::Path::new(p).is_file()).ok_or("No supported terminal was found. Use the manual steps.")?;
    let mut cmd=Command::new(launcher);
    cmd.args(linux_args(launcher,"/bin/sh",&[script.to_str().ok_or("Invalid temporary path.")?.into()],true));
    Ok(cmd)
}

#[cfg(target_os="macos")]
fn public_install_launcher(script:&std::path::Path)->Result<Command,String>{
    const TERMINAL:&str="/System/Applications/Utilities/Terminal.app";
    if !std::path::Path::new(TERMINAL).is_dir(){return Err("Terminal.app was not found. Use the manual steps.".into());}
    let mut cmd=Command::new("/usr/bin/open");
    cmd.args(["-a",TERMINAL]).arg(script);
    Ok(cmd)
}

#[cfg(all(unix,not(any(target_os="linux",target_os="macos"))))]
fn public_install_launcher(_: &std::path::Path)->Result<Command,String>{
    Err("Use the manual public-key steps on this computer.".into())
}

pub fn open(device: &Device, route: Route) -> Result<(), String> {
    open_tracked(device,route).map(|_|())
}

pub fn open_tracked(device:&Device,route:Route)->Result<LaunchWatch,String>{
    spawn(&external_args(device,route)?,None)
}

fn launch(mut cmd: Command) -> Result<LaunchWatch,String> {
    let mut child=cmd.spawn().map_err(|_|"Could not open an external terminal. Use Copy command instead.".to_string())?;
    // Reap the app launcher; not a session supervisor. The user's terminal owns SSH.
    let (tx,rx)=mpsc::channel();
    std::thread::Builder::new().name("homeops-external-terminal".into()).spawn(move ||{
        let outcome=match child.wait(){
            Ok(code) if code.success()=>Ok(()),
            Ok(code)=>Err(format!("External terminal exited ({code}). Review the terminal output or try a regular terminal.")),
            Err(_)=>Err("Could not get the external terminal status.".into()),
        };
        let _=tx.send(outcome);
    }).map_err(|_|"External terminal launch requested, but status could not be monitored.")?;
    Ok(rx)
}

#[cfg(target_os="linux")]
fn spawn(args: &[String], installer: Option<&str>) -> Result<LaunchWatch,String> {
    let ssh=match installer {Some(tool)=>PathBuf::from(tool),None=>system_ssh()?};
    let preferred=(installer.is_some() && std::path::Path::new("/usr/bin/foot").is_file()).then_some("/usr/bin/foot");
    let launcher=preferred.into_iter().chain(["/usr/bin/xdg-terminal-exec","/usr/bin/foot","/usr/bin/kitty","/usr/bin/alacritty","/usr/bin/xterm"])
        .find(|p|std::path::Path::new(p).is_file()).ok_or("No supported external terminal was found. Use Copy command instead.")?;
    let mut cmd=Command::new(launcher);
    cmd.args(linux_args(launcher,ssh.to_str().ok_or("Invalid OpenSSH path.")?,args,installer.is_some()));
    launch(cmd)
}

#[cfg(target_os="macos")]
fn spawn(args: &[String], installer: Option<&str>) -> Result<LaunchWatch,String> {
    use std::{fs::OpenOptions,io::Write,os::unix::fs::OpenOptionsExt};
    let ssh=match installer {Some(tool)=>PathBuf::from(tool),None=>system_ssh()?};
    const TERMINAL:&str="/System/Applications/Utilities/Terminal.app";
    if !std::path::Path::new(TERMINAL).is_dir(){return Err("Terminal.app was not found. Use Copy command instead.".into());}
    let dir=tempfile::Builder::new().prefix("homeops-terminal-").tempdir().map_err(|_|"Cannot prepare Terminal.app launch file. Use Copy command instead.")?;
    let script=dir.path().join("connect.command");
    let line=mac_command(ssh.to_str().ok_or("Invalid OpenSSH path.")?,args);
    let wrapper=format!("#!/bin/sh\n/bin/rm -f -- \"$0\"\n/bin/rmdir -- \"${{0%/*}}\" 2>/dev/null || :\nexec {line}\n");
    let mut f=OpenOptions::new().create_new(true).write(true).mode(0o700).open(&script)
        .map_err(|_|"Cannot prepare Terminal.app launch file. Use Copy command instead.")?;
    f.write_all(wrapper.as_bytes()).map_err(|_|"Cannot write Terminal.app launch file. Use Copy command instead.")?;
    drop(f);
    // LaunchServices may not open it until after /usr/bin/open exits. Keep
    // the private file through handoff; it deletes itself when Terminal runs it.
    let mut cmd=Command::new("/usr/bin/open");
    cmd.args(["-a",TERMINAL]).arg(&script);
    let watching=launch(cmd)?;
    let _=dir.keep();
    Ok(watching)
}

#[cfg(target_os="windows")]
fn windows_powershell()->Result<PathBuf,String>{
    let root=std::env::var_os("SystemRoot").map(PathBuf::from).filter(|p|p.is_absolute())
        .ok_or("Could not locate Windows system programs.")?;
    let ps=root.join("System32").join("WindowsPowerShell").join("v1.0").join("powershell.exe");
    if ps.is_file(){Ok(ps)}else{Err("Windows PowerShell was not found. Use the manual steps.".into())}
}

#[cfg(target_os="windows")]
fn spawn(args: &[String], installer: Option<&str>) -> Result<LaunchWatch,String> {
    use std::os::windows::process::CommandExt;
    if installer.is_some(){return Err("Use the manual public-key installation steps on Windows.".into());}
    let root=std::env::var_os("SystemRoot").map(PathBuf::from).filter(|p|p.is_absolute()).ok_or("Could not locate Windows system programs. Use Copy command instead.")?;
    let dir=root.join("System32");
    let ssh=dir.join("OpenSSH").join("ssh.exe");
    let ps=dir.join("WindowsPowerShell").join("v1.0").join("powershell.exe");
    if !ssh.is_file(){return Err("Windows OpenSSH was not found. Use Copy command instead.".into());}
    if !ps.is_file(){return Err("Windows PowerShell was not found. Use Copy command instead.".into());}
    // A persistent console keeps SSH's diagnostic (including changed-key errors) visible.
    // Quote the fixed executable plus each device argument independently; no shell input is accepted.
    let mut cmd=Command::new(ps);
    cmd.creation_flags(0x10).args(["-NoLogo","-NoProfile","-NoExit","-Command"])
       .arg(powershell_command(ssh.to_str().ok_or("Invalid OpenSSH path.")?,args));
    launch(cmd)
}

#[cfg(not(any(target_os="windows",target_os="macos",target_os="linux")))]
fn spawn(_: &[String], _: Option<&str>) -> Result<LaunchWatch,String> {
    Err("This system has no external terminal integration. Use Copy command instead.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_selected_route_username_and_port_through_linux_argv() {
        let d=fixture();
        let argv=external_args(&d,Route::Lan).unwrap();
        assert_eq!(argv,["-oStrictHostKeyChecking=ask","-l","fixture_user","-p","2202","--","2001:db8::2"]);
        assert_eq!(linux_args("/usr/bin/xdg-terminal-exec","/usr/bin/ssh",&argv,false),
            ["--","/usr/bin/ssh","-oStrictHostKeyChecking=ask","-l","fixture_user","-p","2202","--","2001:db8::2"]);
        assert_eq!(linux_args("/usr/bin/foot","/usr/bin/ssh",&argv,false)[0],"-e");
        assert_eq!(&linux_args("/usr/bin/foot","/usr/bin/ssh-copy-id",&argv,true)[..3],["--hold","-e","/usr/bin/ssh-copy-id"]);
        assert_eq!(external_args(&d,Route::Tailscale).unwrap().last().unwrap(),"node.example");
    }
    #[test]
    fn rejects_invalid_input_before_quoting_and_keeps_fixed_arguments_separate() {
        let mut d=fixture();
        d.user="x'; reboot".into();
        assert!(external_args(&d,Route::Lan).is_err());
        let argv=external_args(&fixture(),Route::Lan).unwrap();
        assert_eq!(mac_command("/usr/bin/ssh",&argv),"'/usr/bin/ssh' '-oStrictHostKeyChecking=ask' '-l' 'fixture_user' '-p' '2202' '--' '2001:db8::2'");
        assert_eq!(powershell_command("C:\\Program Files\\ssh.exe",&argv),"& 'C:\\Program Files\\ssh.exe' '-oStrictHostKeyChecking=ask' '-l' 'fixture_user' '-p' '2202' '--' '2001:db8::2'");
    }
    #[test]
    fn public_install_sets_strict_direct_host_trust_and_one_fixed_snapshot(){
        let args=public_install_args(&fixture(),Route::Lan,"/tmp/drakops-key-fixture/selected.pub").unwrap();
        assert_eq!(&args[..5],["-f","-i","/tmp/drakops-key-fixture/selected.pub","-F","/dev/null"]);
        assert_eq!(&args[args.len()-3..],["-p","2202","fixture_user@2001:db8::2"]);
        for option in ["-oStrictHostKeyChecking=yes","-oUpdateHostKeys=no","-oControlPath=none","-oForwardAgent=no","-oForwardX11=no"]{
            assert!(args.iter().any(|a|a==option));
        }
        assert!(public_install_args(&fixture(),Route::Lan,"/tmp/drakops-key-fixture with space/selected.pub").is_err());
        let wrapper=public_install_wrapper("/usr/bin/ssh-copy-id",&args,100);
        assert!(wrapper.contains("trap cleanup 0"));
        assert!(wrapper.contains("Setup request expired"));
        assert!(wrapper.contains("'/usr/bin/ssh-copy-id' '-f' '-i' '/tmp/drakops-key-fixture/selected.pub'"));
    }

    #[cfg(unix)]
    #[test]
    fn public_install_wrapper_keeps_snapshot_through_child_and_cleans_it_afterward(){
        let stage=tempfile::tempdir().unwrap();
        let pubfile=stage.path().join("selected.pub");
        let script=stage.path().join("install.command");
        std::fs::write(&pubfile,"synthetic review bytes\n").unwrap();
        let argv=vec!["-c".into(),"test -r \"$1\" && test \"$(cat \"$1\")\" = 'synthetic review bytes'".into(),
            "fixture".into(),pubfile.to_str().unwrap().into()];
        std::fs::write(&script,public_install_wrapper("/bin/sh",&argv,u64::MAX/4)).unwrap();
        let status=Command::new("/bin/sh").arg(&script).status().unwrap();
        assert!(status.success());
        assert!(!pubfile.exists() && !script.exists() && !stage.path().exists());
    }

    fn fixture() -> Device {
        Device{alias:"node".into(),lan:"2001:db8::2".into(),tailscale:"node.example".into(),user:"fixture_user".into(),port:2202,..Default::default()}
    }
}
