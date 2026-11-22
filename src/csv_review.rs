use crate::book::{self, Device};
use std::{
    path::Path,
    sync::mpsc::{self, Receiver, TryRecvError},
};

pub(crate) type ReviewResult = Result<Vec<Device>, String>;

#[derive(Default)]
pub(crate) struct CsvReview {
    // Cancelled I/O retains its one slot until the operating system returns.
    // This avoids spawning unbounded threads if a path repeatedly stalls.
    job: Option<Job>,
}

struct Job {
    result: Receiver<ReviewResult>,
    current: bool,
}

fn read_csv(path: &Path) -> ReviewResult {
    let file=book::open_regular(path).map_err(|e|e.to_string())?;
    let bytes=book::bounded(file,1024*1024)?;
    book::parse_csv(&bytes)
}

impl CsvReview {
    pub(crate) fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub(crate) fn current(&self) -> bool {
        self.job.as_ref().is_some_and(|job| job.current)
    }

    pub(crate) fn invalidate(&mut self) {
        if let Some(job) = &mut self.job {
            job.current = false;
        }
    }

    pub(crate) fn start(&mut self, path: &str) -> Result<(), String> {
        if self.busy() {
            return Err("The last CSV read is still finishing.".into());
        }
        let path = Path::new(path.trim()).to_owned();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new().name("drakops-csv-review".into()).spawn(move || {
            let _ = tx.send(read_csv(&path));
        }).map_err(|_| "Could not start the CSV review.".to_owned())?;
        self.job = Some(Job { result: rx, current: true });
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> Option<ReviewResult> {
        let job = self.job.as_ref()?;
        let result = match job.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("The CSV review stopped early.".into()),
        };
        let current = job.current;
        self.job = None;
        current.then_some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn regular_csv_files_are_bounded_and_validate_the_account() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hosts.csv");
        fs::write(&file, "alias,hostname,user,port\nfixture,example.test,admin,2222\n").unwrap();
        let devices = read_csv(&file).unwrap();
        assert_eq!(devices[0].user, "admin");
        assert!(read_csv(dir.path()).is_err());
        fs::File::create(&file).unwrap().set_len(1_048_577).unwrap();
        assert_eq!(read_csv(&file).unwrap_err(), "File exceeds the size limit.");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_fifo_without_a_writer() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("not-a-csv");
        let c_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        assert_eq!(read_csv(&fifo).unwrap_err(),"Choose a regular file.");
        assert_eq!(book::open_regular(&fifo).unwrap_err().kind(),std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn cancelled_or_replaced_path_cannot_publish_an_old_result() {
        let (tx, rx) = mpsc::channel();
        let mut state = CsvReview { job: Some(Job { result: rx, current: true }) };
        assert!(state.poll().is_none());
        state.invalidate();
        assert!(state.busy());
        assert!(!state.current());
        assert!(state.start("new.csv").is_err());
        tx.send(Ok(vec![Device::default()])).unwrap();
        assert!(state.poll().is_none());
        assert!(!state.busy());
    }
}
