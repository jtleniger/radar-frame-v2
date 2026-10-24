//! Size-based log rotation matching the server's `radar-frame.log` layout.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::fmt::MakeWriter;

const MAX_LOG_BYTES: u64 = 100_000;
const LOG_BACKUPS: usize = 4;

/// Install the global logger. A log file that cannot be opened falls back to
/// stderr (the systemd journal), like the server.
pub fn init(path: &Path, level: &str) -> Result<(), String> {
    let filter = level
        .parse::<tracing::level_filters::LevelFilter>()
        .map_err(|_| format!("invalid log level {level:?}"))?;

    let sink = match RotatingFile::open(path, MAX_LOG_BYTES, LOG_BACKUPS) {
        Ok(file) => Sink::File(file),
        Err(error) => {
            eprintln!("{}: {error}; logging to stderr", path.display());
            Sink::Stderr
        }
    };

    tracing_subscriber::fmt()
        .with_writer(SharedSink(Arc::new(Mutex::new(sink))))
        .with_max_level(filter)
        .with_ansi(false)
        .with_target(true)
        .with_file(true)
        .with_line_number(true)
        .with_timer(UtcTimer)
        .init();
    Ok(())
}

#[derive(Clone, Copy)]
struct UtcTimer;

impl FormatTime for UtcTimer {
    fn format_time(
        &self,
        writer: &mut tracing_subscriber::fmt::format::Writer<'_>,
    ) -> std::fmt::Result {
        write!(
            writer,
            "{}",
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
        )
    }
}

#[derive(Clone)]
struct SharedSink(Arc<Mutex<Sink>>);

struct SinkWriter<'a>(MutexGuard<'a, Sink>);

impl Write for SinkWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<'a> MakeWriter<'a> for SharedSink {
    type Writer = SinkWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        SinkWriter(self.0.lock().unwrap_or_else(|error| error.into_inner()))
    }
}

enum Sink {
    File(RotatingFile),
    Stderr,
}

impl Write for Sink {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Sink::File(file) => file.write(buffer),
            Sink::Stderr => io::stderr().write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Sink::File(file) => file.flush(),
            Sink::Stderr => io::stderr().flush(),
        }
    }
}

struct RotatingFile {
    path: PathBuf,
    file: File,
    len: u64,
    max_bytes: u64,
    backups: usize,
}

impl RotatingFile {
    fn open(path: &Path, max_bytes: u64, backups: usize) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let len = file.metadata()?.len();
        Ok(RotatingFile {
            path: path.to_path_buf(),
            file,
            len,
            max_bytes,
            backups: backups.max(1),
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        for index in (1..self.backups).rev() {
            let from = backup_path(&self.path, index);
            let to = backup_path(&self.path, index + 1);
            if from.exists() {
                fs::rename(from, to)?;
            }
        }

        fs::rename(&self.path, backup_path(&self.path, 1))?;
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.len = 0;
        Ok(())
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.len > 0 && self.len.saturating_add(buffer.len() as u64) > self.max_bytes {
            // A failed rotation must not take the service down. Keep appending
            // to the current file and try again on the next write.
            let _ = self.rotate();
        }
        let written = self.file.write(buffer)?;
        self.len = self.len.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn backup_path(path: &Path, index: usize) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{index}"));
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_at_the_size_limit_and_keeps_backups() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("radar-frame.log");
        let mut file = RotatingFile::open(&path, 10, 2).unwrap();

        file.write_all(b"12345678").unwrap();
        file.write_all(b"abcdefgh").unwrap();
        file.write_all(b"ABCDEFGH").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"ABCDEFGH");
        assert_eq!(fs::read(backup_path(&path, 1)).unwrap(), b"abcdefgh");
        assert_eq!(fs::read(backup_path(&path, 2)).unwrap(), b"12345678");
    }
}
