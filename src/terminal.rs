//! Single-session interactive OpenSSH PTY. No transcript or outbound terminal side effects.
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{self, Read, Write}, path::PathBuf,
    sync::{
        Arc, atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread, time::Duration,
};

pub const MAX_ROWS: u16 = 64;
pub const MAX_COLS: u16 = 200;
pub const SCROLLBACK: usize = 200;
pub const MAX_INPUT: usize = 4096;
const FRAME: usize = 4096;
const QUEUE: usize = 32;
const OUTPUT_PER_POLL: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State { Starting, Running, Disconnecting, Disconnected, Exited(u32), Failed(String) }
impl State {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Disconnecting)
    }
    pub fn label(&self) -> String {
        match self {
            Self::Starting => "Starting OpenSSH".into(),
            Self::Running => "OpenSSH running".into(),
            Self::Disconnecting => "Disconnecting".into(),
            Self::Disconnected => "Disconnected".into(),
            Self::Exited(code) => format!("OpenSSH exited ({code})"),
            Self::Failed(message) => message.clone(),
        }
    }
}

enum WorkerEvent { Started, Done(State) }

pub struct Session {
    // Parser<()> has no clipboard, bell, window title, or URI callbacks.
    parser: vt100::Parser<()>,
    state: State,
    cancel: Arc<AtomicBool>,
    size: Arc<AtomicU32>,
    input: Option<SyncSender<Vec<u8>>>,
    output: Option<Receiver<Vec<u8>>>,
    events: Receiver<WorkerEvent>,
}

impl Session {
    /// Accept only Device::ssh_args canonical form, not arbitrary SSH options.
    pub fn start(args: &[String], rows: u16, cols: u16) -> Result<Self, String> {
        let args = ssh_arguments(args)?;
        let exe = system_ssh()?;
        let mut cmd = CommandBuilder::new(exe);
        cmd.args(args);
        cmd.env("TERM", "xterm-256color");
        cmd.env("SSH_ASKPASS_REQUIRE", "never");
        Self::spawn(cmd, rows, cols)
    }
    fn spawn(cmd: CommandBuilder, rows: u16, cols: u16) -> Result<Self, String> {
        let (rows, cols) = dimensions(rows, cols);
        let cancel = Arc::new(AtomicBool::new(false));
        let size = Arc::new(AtomicU32::new(pack(rows, cols)));
        let (in_tx, in_rx) = mpsc::sync_channel(QUEUE);
        let (out_tx, out_rx) = mpsc::sync_channel(QUEUE);
        let (events_tx, events_rx) = mpsc::channel();
        let worker_cancel = Arc::clone(&cancel);
        let worker_size = Arc::clone(&size);

        thread::Builder::new().name("homeops-ssh-supervisor".into()).spawn(move || {
            let done = supervise(cmd, &worker_cancel, &worker_size, in_rx, out_tx, &events_tx);
            let _ = events_tx.send(WorkerEvent::Done(done));
        }).map_err(|_| "Could not start the terminal worker. Use an external terminal.".to_owned())?;

        Ok(Self {
            parser: vt100::Parser::new(rows, cols, SCROLLBACK),
            state: State::Starting, cancel, size,
            input: Some(in_tx), output: Some(out_rx), events: events_rx,
        })
    }
    /// Called from UI frames; bounded parser work, no locks or external I/O.
    pub fn poll(&mut self) {
        if let Some(out) = self.output.as_ref() {
            for _ in 0..OUTPUT_PER_POLL {
                let Ok(bytes) = out.try_recv() else { break };
                self.parser.process(&bytes);
            }
        }
        loop {
            match self.events.try_recv() {
                Ok(WorkerEvent::Started) if self.state == State::Starting => self.state = State::Running,
                Ok(WorkerEvent::Started) => {}
                Ok(WorkerEvent::Done(s)) => { self.state = s; self.input.take(); }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if self.state.is_active() {
                        self.input.take();
                        self.state = State::Failed("Terminal worker stopped unexpectedly.".into());
                    }
                    break;
                }
            }
        }
    }
    pub fn state(&self) -> &State { &self.state }
    pub fn screen(&self) -> &vt100::Screen { self.parser.screen() }
    pub fn set_scrollback(&mut self, rows: usize) { self.parser.screen_mut().set_scrollback(rows.min(SCROLLBACK)); }

    /// Queue only intentional user input. Never send a parser response.
    pub fn send(&self, bytes: &[u8]) -> Result<(), String> {
        if self.state != State::Running {
            return Err("Terminal is not running.".into());
        }
        if bytes.len() > MAX_INPUT {
            return Err("Input is too large for one terminal write.".into());
        }
        let tx = self.input.as_ref().ok_or("Terminal input is closed.")?;
        tx.try_send(bytes.to_vec()).map_err(|e| match e {
            TrySendError::Full(_) => "Terminal is busy. Try again.".into(),
            TrySendError::Disconnected(_) => "Terminal input is closed.".into(),
        })
    }
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<(), String> {
        let (rows, cols) = dimensions(rows, cols);
        if self.parser.screen().size() != (rows, cols) {
            self.parser.screen_mut().set_size(rows, cols);
            self.size.store(pack(rows, cols), Ordering::Release);
        }
        Ok(())
    }
    pub fn disconnect(&mut self) {
        if !self.state.is_active() { return; }
        self.cancel.store(true, Ordering::Release);
        self.state = State::Disconnecting;
        self.input.take();
        // A disconnected reader keeps draining and discarding until its PTY closes.
        self.output.take();
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.input.take();
        self.output.take();
    }
}

fn dimensions(rows: u16, cols: u16) -> (u16, u16) {
    (rows.clamp(2, MAX_ROWS), cols.clamp(2, MAX_COLS))
}
fn pack(rows: u16, cols: u16) -> u32 { (u32::from(rows) << 16) | u32::from(cols) }
fn winsize(packed: u32) -> PtySize {
    PtySize { rows: (packed >> 16) as u16, cols: packed as u16, pixel_width: 0, pixel_height: 0 }
}
fn failure(s: &str) -> State { State::Failed(format!("{s} Use an external terminal.")) }

fn supervise(
    cmd: CommandBuilder, cancel: &AtomicBool, size: &AtomicU32,
    input: Receiver<Vec<u8>>, output: SyncSender<Vec<u8>>, events: &mpsc::Sender<WorkerEvent>,
) -> State {
    if cancel.load(Ordering::Acquire) { return State::Disconnected; }
    let mut previous_size = size.load(Ordering::Acquire);
    let Ok(pair) = native_pty_system().openpty(winsize(previous_size)) else {
        return failure("The system terminal backend is unavailable.");
    };
    let Ok(mut reader) = pair.master.try_clone_reader() else { return failure("Could not open terminal output."); };
    let Ok(mut writer) = pair.master.take_writer() else { return failure("Could not open terminal input."); };

    // Start the drain before spawn: ConPTY can synchronously flush on teardown.
    let reader_thread = thread::Builder::new().name("homeops-ssh-reader".into()).spawn(move || {
        let mut frame = [0u8; FRAME];
        let mut deliver = true;
        loop {
            match reader.read(&mut frame) {
                Ok(0) => break,
                Ok(n) if deliver => deliver = output.send(frame[..n].to_vec()).is_ok(),
                Ok(_) => {},
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break, // PTYs may report an I/O error rather than EOF on teardown.
            }
        }
    });
    if reader_thread.is_err() { return failure("Could not start terminal output."); }

    if cancel.load(Ordering::Acquire) { return State::Disconnected; }
    let child = pair.slave.spawn_command(cmd);
    drop(pair.slave);
    let Ok(mut child) = child else { return failure("Could not launch OpenSSH."); };
    let writer_thread = thread::Builder::new().name("homeops-ssh-writer".into()).spawn(move || {
        for bytes in input {
            if writer.write_all(&bytes).and_then(|_| writer.flush()).is_err() { break; }
        }
    });

    let result = if writer_thread.is_err() {
        failure("Could not start terminal input.")
    } else {
        let _ = events.send(WorkerEvent::Started);
        loop {
            if cancel.load(Ordering::Acquire) { break State::Disconnected; }
            match child.try_wait() {
                Ok(Some(status)) => break State::Exited(status.exit_code()),
                Ok(None) => {},
                Err(_) => break failure("Could not read the terminal exit status."),
            }
            let next_size = size.load(Ordering::Acquire);
            if next_size != previous_size {
                if pair.master.resize(winsize(next_size)).is_err() {
                    break failure("The terminal could not resize.");
                }
                previous_size = next_size;
            }
            thread::sleep(Duration::from_millis(20));
        }
    };
    // Natural exit already reaped by try_wait. All other branches own cleanup.
    if !matches!(result, State::Exited(_)) {
        if !matches!(child.try_wait(), Ok(Some(_))) {
            if child.kill().is_err() && !matches!(child.try_wait(), Ok(Some(_))) {
                return failure("Could not close the OpenSSH process.");
            }
            if child.wait().is_err() { return failure("Could not reap the OpenSSH process."); }
        }
    }
    drop(pair.master);
    result
}

fn system_ssh() -> Result<PathBuf, String> {
    #[cfg(target_os="windows")]
    let paths = {
        let root = std::env::var_os("SystemRoot").map(PathBuf::from)
            .filter(|p| p.is_absolute()).ok_or("Could not locate Windows OpenSSH.")?;
        vec![root.join("System32").join("OpenSSH").join("ssh.exe")]
    };
    #[cfg(not(target_os="windows"))]
    let paths = vec![PathBuf::from("/usr/bin/ssh"), PathBuf::from("/bin/ssh")];
    paths.into_iter().find(|p| p.is_file())
        .ok_or_else(|| "System OpenSSH was not found. Use an external terminal.".into())
}

fn ssh_arguments(args: &[String]) -> Result<Vec<String>, String> {
    let invalid = || "Invalid SSH connection settings.".to_owned();
    let (user, port, host) = match args {
        [l, u, dash, h] if l == "-l" && dash == "--" => (u, 22, h),
        [l, u, p, n, dash, h] if l == "-l" && p == "-p" && dash == "--" => {
            (u, n.parse::<u16>().ok().filter(|n| *n != 0).ok_or_else(invalid)?, h)
        }
        _ => return Err(invalid()),
    };
    let username_ok = !user.is_empty() && user.len() <= 64
        && !user.starts_with('-') && user.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    let address_ok = host.len() <= 253 && (host.parse::<std::net::IpAddr>().is_ok()
        || (host.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
            && host.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))));
    if !username_ok || !address_ok { return Err(invalid()); }
    let mut options: Vec<String> = [
        "-tt", "-a", "-oStrictHostKeyChecking=yes", "-oUpdateHostKeys=no",
        "-oPermitLocalCommand=no", "-oRemoteCommand=none",
        "-oControlPath=none", "-oControlMaster=no", "-oControlPersist=no",
    ].into_iter().map(str::to_owned).collect();
    options.extend(["-p".into(), port.to_string(), "-l".into(), user.clone(), "--".into(), host.clone()]);
    Ok(options)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::time::Instant;

    fn strings(args: &[&str]) -> Vec<String> { args.iter().map(|s| (*s).into()).collect() }
    #[test]
    fn pinned_destination_and_security_options() {
        let got = ssh_arguments(&strings(&["-l", "alex", "-p", "2222", "--", "2001:db8::2"])).unwrap();
        assert_eq!(&got[got.len()-6..], ["-p","2222","-l","alex","--","2001:db8::2"]);
        for setting in ["-tt","-a","-oStrictHostKeyChecking=yes","-oUpdateHostKeys=no","-oRemoteCommand=none","-oControlPath=none"] {
            assert!(got.iter().any(|s| s == setting));
        }
    }
    #[test]
    fn rejects_malformed_destinations_and_extra_options() {
        for args in [
            &["-l","alex","--","-oProxyCommand=bad"][..],
            &["-l","alex","--","host","command"],
            &["-l","alex@elsewhere","--","good.example"],
            &["-l","alex","-p","0","--","good.example"],
            &["-l","alex","-p","99999","--","good.example"],
            &["-l","alex","--","good\nbad"],
            &["-l","alex","--","good;bad"],
        ] { assert!(ssh_arguments(&strings(args)).is_err(), "{args:?}"); }
    }
    #[test]
    fn split_control_strings_have_no_screen_effect_and_cells_render() {
        let mut model = vt100::Parser::new(12, 80, SCROLLBACK);
        for chunk in [
            b"hello \x1b]52;c;Y2xpcGJvYXJk".as_slice(),
            b"\x07\x1b]52;c;?\x1b\\\x1b]8;;https://never".as_slice(),
            b".invalid\x1b\\link text\x1b]8;;\x1b\\\r\n\x1b[31mR\x1b[m\x1b]2;title".as_slice(),
            b"\x07 \xe7".as_slice(),
            b"\x95\x8c".as_slice(),
        ] { model.process(chunk); }
        let s = model.screen();
        assert!(s.contents().contains("hello link text"));
        assert!(!s.contents().contains("title"));
        assert!(!s.contents().contains("never.invalid"));
        assert_eq!(s.cell(1,0).unwrap().fgcolor(), vt100::Color::Idx(1));
        assert_eq!(s.cell(1,2).unwrap().contents(), "界");
    }

    #[test]
    fn pty_fixture_child() {
        if std::env::var("HOMEOPS_TERMINAL_FIXTURE").as_deref() != Ok("1") { return; }
        println!("[fixture-ready]");
        io::stdout().flush().unwrap();
        let mut line = String::new();
        io::stdin().read_line(&mut line).unwrap();
        println!("[fixture-got:{}]", line.trim_end());
        io::stdout().flush().unwrap();
        if line.trim() == "wait" { let _ = io::stdin().read_line(&mut line); }
    }
    pub(crate) fn fixture() -> Session {
        let mut cmd = CommandBuilder::new(std::env::current_exe().unwrap());
        cmd.args(["--exact","terminal::tests::pty_fixture_child","--nocapture"]);
        cmd.env("HOMEOPS_TERMINAL_FIXTURE","1");
        Session::spawn(cmd,24,80).unwrap()
    }
    pub(crate) fn until(session: &mut Session, cond: impl Fn(&Session) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            session.poll();
            if cond(session) { return; }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("fixture did not reach expected state ({:?}), screen: {:?}", session.state(), session.screen().contents());
    }
    #[test]
    fn pty_roundtrip_resize_natural_exit_and_disconnect() {
        let mut s = fixture();
        until(&mut s, |s| s.state() == &State::Running && s.screen().contents().contains("[fixture-ready]"));
        s.resize(31, 101).unwrap();
        assert_eq!(s.screen().size(), (31,101));
        assert!(s.send(&vec![b'x'; MAX_INPUT+1]).is_err());
        s.send(b"hello\r").unwrap();
        until(&mut s, |s| s.state()==&State::Exited(0) && s.screen().contents().contains("[fixture-got:hello]"));

        let mut s = fixture();
        until(&mut s, |s| s.state() == &State::Running && s.screen().contents().contains("[fixture-ready]"));
        s.send(b"wait\r").unwrap();
        until(&mut s, |s| s.screen().contents().contains("[fixture-got:wait]"));
        s.disconnect();
        s.disconnect();
        assert!(s.send(b"no").is_err());
        until(&mut s, |s| s.state() == &State::Disconnected);
    }
    #[test]
    fn cancel_while_starting_is_clean() {
        let mut s = fixture();
        s.disconnect();
        until(&mut s, |s| s.state() == &State::Disconnected);
    }
}
