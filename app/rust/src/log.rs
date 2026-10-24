//! The app's log file: what Anvil did, kept beside the session in the app's
//! support directory so a user can send it when a call goes wrong. The file
//! is capped: past [`MAX_BYTES`] it becomes `anvil.log.1` (replacing the one
//! before) and a new one starts, so at most twice that is kept.
//!
//! `ANVIL_LOG` (a `tracing` filter, e.g. `debug`) changes what is written.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use parking_lot::{Mutex, MutexGuard};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

/// The size a log file grows to before it is rotated.
pub(crate) const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// The file's name in the support directory.
pub(crate) const FILE_NAME: &str = "anvil.log";

/// Anvil's own crates at INFO, everything else only when it warns.
const DEFAULT_FILTER: &str = "warn,anvil_core=info,anvil_app=info,anvil_fcp=info,\
                              anvil_audio=info,anvil_brand=info,anvil_update=info,\
                              anvil_bridge=info";

static PATH: OnceLock<PathBuf> = OnceLock::new();

/// Start writing the log into `dir`, once; the file's path. Later calls
/// return the path the first one chose.
pub(crate) fn start(dir: &Path) -> io::Result<PathBuf> {
    if let Some(path) = PATH.get() {
        return Ok(path.clone());
    }
    std::fs::create_dir_all(dir)?;
    let path = dir.join(FILE_NAME);
    let writer = SizeCapped::open(path.clone(), MAX_BYTES)?;
    let filter = EnvFilter::try_from_env("ANVIL_LOG").unwrap_or_else(|_| DEFAULT_FILTER.into());
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true);
    // Something else may own the global subscriber (a test harness): the
    // file is then not written, which is no reason to fail the app.
    let installed = tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init()
        .is_ok();
    let _ = PATH.set(path.clone());
    if installed {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            tracing::error!(%info, "panic");
            previous(info);
        }));
        tracing::info!(
            version = env!("CARGO_PKG_VERSION"),
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
            "Anvil started"
        );
    }
    Ok(path)
}

/// The log file's path, once [`start`]ed.
pub(crate) fn path() -> Option<PathBuf> {
    PATH.get().cloned()
}

/// A file that rotates to `<name>.1` when it would pass `max` bytes.
pub(crate) struct SizeCapped {
    path: PathBuf,
    max: u64,
    state: Mutex<State>,
}

struct State {
    file: Option<File>,
    written: u64,
}

impl SizeCapped {
    pub(crate) fn open(path: PathBuf, max: u64) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            path,
            max,
            state: Mutex::new(State {
                file: Some(file),
                written,
            }),
        })
    }

    fn rotated(&self) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(".1");
        PathBuf::from(name)
    }
}

impl State {
    /// Close the file (Windows renames no open file), move it to `old`,
    /// start a new one.
    fn rotate(&mut self, path: &Path, old: &Path) -> io::Result<()> {
        self.file = None;
        let _ = std::fs::remove_file(old);
        std::fs::rename(path, old)?;
        self.file = Some(OpenOptions::new().create(true).append(true).open(path)?);
        self.written = 0;
        Ok(())
    }
}

/// One event's write, under the file's lock.
pub(crate) struct Guard<'a> {
    owner: &'a SizeCapped,
    state: MutexGuard<'a, State>,
}

impl Write for Guard<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let state = &mut *self.state;
        if state.written > 0 && state.written + buf.len() as u64 > self.owner.max {
            let rotated = state.rotate(&self.owner.path, &self.owner.rotated());
            if rotated.is_err() && state.file.is_none() {
                // Keep writing where we can rather than lose the log.
                state.file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.owner.path)
                    .ok();
            }
        }
        let Some(file) = state.file.as_mut() else {
            return Ok(buf.len());
        };
        let n = file.write(buf)?;
        state.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.state.file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

impl<'a> MakeWriter<'a> for SizeCapped {
    type Writer = Guard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        Guard {
            owner: self,
            state: self.state.lock(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "anvil-log-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_file_rotates_past_its_cap_and_keeps_one_old() {
        let dir = dir("rotate");
        let path = dir.join(FILE_NAME);
        let log = SizeCapped::open(path.clone(), 100).unwrap();
        let line = [b'a'; 40];
        for _ in 0..2 {
            log.make_writer().write_all(&line).unwrap();
        }
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 80);
        // The third would pass 100: the first two move to .1.
        log.make_writer().write_all(&[b'b'; 40]).unwrap();
        let old = dir.join(format!("{FILE_NAME}.1"));
        assert_eq!(std::fs::read(&old).unwrap(), [b'a'; 80]);
        assert_eq!(std::fs::read(&path).unwrap(), [b'b'; 40]);
        // And again: the older one is gone.
        for _ in 0..2 {
            log.make_writer().write_all(&[b'c'; 40]).unwrap();
        }
        let mut expected = vec![b'b'; 40];
        expected.extend([b'c'; 40]);
        assert_eq!(std::fs::read(&old).unwrap(), expected);
        assert_eq!(std::fs::read(&path).unwrap(), [b'c'; 40]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_already_past_its_cap_rotates_at_the_first_write() {
        let dir = dir("reopen");
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, [b'x'; 150]).unwrap();
        let log = SizeCapped::open(path.clone(), 100).unwrap();
        log.make_writer().write_all(b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(
            std::fs::metadata(dir.join(format!("{FILE_NAME}.1")))
                .unwrap()
                .len(),
            150
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
