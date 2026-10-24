//! File logging with the same size rotation as the Python service:
//! `radar-frame.log`, 100 kB per file, four backups.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;

use crate::config::paths::Paths;

const MAX_BYTES: u64 = 100_000;
const BACKUPS: usize = 4;

pub struct RotatingWriter {
    state: Arc<Mutex<WriterState>>,
}

struct WriterState {
    path: PathBuf,
    file: File,
    length: u64,
    max_bytes: u64,
    backups: usize,
    /// Length at which rotation is retried after a failure. Zero means the
    /// normal `max_bytes` threshold; otherwise the file grows for one more
    /// window instead of retrying (and warning) on every write.
    retry_at: u64,
}

impl RotatingWriter {
    pub fn new(path: PathBuf, max_bytes: u64, backups: usize) -> io::Result<Self> {
        let file = open(&path)?;
        let length = file.metadata()?.len();
        Ok(RotatingWriter {
            state: Arc::new(Mutex::new(WriterState {
                path,
                file,
                length,
                max_bytes,
                backups,
                retry_at: 0,
            })),
        })
    }

    fn write_all(&self, buffer: &[u8]) -> io::Result<()> {
        // Report outside the lock: a tracing call while holding the writer
        // mutex would recurse into this writer and deadlock.
        let warning = {
            let mut state = self.state.lock().unwrap();
            state.append(buffer)?
        };
        if let Some(warning) = warning {
            eprintln!("radar-frame: {warning}");
        }
        Ok(())
    }

    fn flush(&self) -> io::Result<()> {
        self.state.lock().unwrap().file.flush()
    }
}

fn open(path: &std::path::Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

impl WriterState {
    /// Append one buffer, rotating first when the current file is full.
    ///
    /// A failed rotation is reported instead of silently ignored; the write
    /// still succeeds so logging does not stop, and the next attempt waits
    /// until another [`Self::max_bytes`] have been written.
    fn append(&mut self, buffer: &[u8]) -> io::Result<Option<String>> {
        let limit = if self.retry_at > self.max_bytes {
            self.retry_at
        } else {
            self.max_bytes
        };
        let mut warning = None;

        if self.length > 0 && self.length.saturating_add(buffer.len() as u64) > limit {
            match self.rotate() {
                Ok(()) => self.retry_at = 0,
                Err(error) => {
                    self.length = self
                        .file
                        .metadata()
                        .map(|metadata| metadata.len())
                        .unwrap_or(self.length);
                    self.retry_at = self.length.saturating_add(self.max_bytes);
                    warning = Some(format!(
                        "rotating {} failed: {error}; continuing to append",
                        self.path.display()
                    ));
                }
            }
        }

        self.file.write_all(buffer)?;
        self.length = self.length.saturating_add(buffer.len() as u64);
        Ok(warning)
    }

    fn rotate(&mut self) -> io::Result<()> {
        if self.backups == 0 {
            self.file = open(&self.path)?;
            self.length = 0;
            return Ok(());
        }

        // RotatingFileHandler drops the oldest backup, shifts the rest up, and
        // reopens the base file.
        let base = self.path.clone();
        let backup = |number: usize| {
            let mut name = base.as_os_str().to_owned();
            name.push(format!(".{number}"));
            PathBuf::from(name)
        };

        match fs::remove_file(backup(self.backups)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        for number in (1..self.backups).rev() {
            match fs::rename(backup(number), backup(number + 1)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        fs::rename(&self.path, backup(1))?;

        self.file = open(&self.path)?;
        self.length = 0;
        Ok(())
    }
}

pub struct Writer<'a>(&'a RotatingWriter);

impl Write for Writer<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write_all(buffer)?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<'a> MakeWriter<'a> for RotatingWriter {
    type Writer = Writer<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        Writer(self)
    }
}

pub fn init(paths: &Paths, level: &str) {
    let filter = LevelFilter::from_str(level).unwrap_or(LevelFilter::INFO);
    let builder = tracing_subscriber::fmt()
        .with_max_level(filter)
        .with_ansi(false);

    // A missing or unwritable log directory must not leave the process
    // without a log sink: keep running with stderr (systemd's journal).
    let initialized = match RotatingWriter::new(paths.log(), MAX_BYTES, BACKUPS) {
        Ok(writer) => builder.with_writer(writer).try_init(),
        Err(error) => {
            eprintln!(
                "radar-frame: cannot open {}: {error}; logging to stderr",
                paths.log().display()
            );
            builder.with_writer(io::stderr).try_init()
        }
    };

    if let Err(error) = initialized {
        eprintln!("radar-frame: logging: {error}");
    }

    install_panic_hook();
}

/// Log panics through tracing; without this the process dies with only the
/// default stderr message and no timestamped record.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let message = if let Some(message) = info.payload().downcast_ref::<&str>() {
            (*message).to_string()
        } else if let Some(message) = info.payload().downcast_ref::<String>() {
            message.clone()
        } else {
            "non-string panic payload".to_string()
        };
        let location = info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "unknown".to_string());

        tracing::error!(
            event = "panic",
            location = %location,
            message = %message,
            backtrace = %std::backtrace::Backtrace::capture(),
            "unhandled panic"
        );
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_rotation_is_reported_and_keeps_appending() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("radar-frame.log");
        let writer = RotatingWriter::new(path.clone(), 16, 1).unwrap();

        // A directory in the backup slot makes the remaining rename fail.
        std::fs::create_dir(dir.path().join("radar-frame.log.1")).unwrap();

        {
            let mut state = writer.state.lock().unwrap();
            assert!(state.append(b"0123456789abcdef").unwrap().is_none());
        }

        let warning = {
            let mut state = writer.state.lock().unwrap();
            state.append(b"overflow").unwrap()
        };

        assert!(
            warning.is_some_and(
                |warning| warning.contains("rotating") && warning.contains("radar-frame.log")
            ),
            "the rotation failure must be reported"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "0123456789abcdefoverflow",
            "a failed rotation must not truncate the log"
        );

        let state = writer.state.lock().unwrap();
        assert_eq!(state.length, 24);
        assert!(
            state.retry_at > state.max_bytes,
            "the next rotation attempt must wait for another window"
        );
    }
}
