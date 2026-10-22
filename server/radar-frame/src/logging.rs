//! File logging with the same size rotation as the Python service:
//! `radar-frame.log`, 100 kB per file, four backups.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;

use crate::paths::Paths;

const MAX_BYTES: u64 = 100_000;
const BACKUPS: usize = 4;

pub struct RotatingWriter {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    path: PathBuf,
    file: File,
    length: u64,
    max_bytes: u64,
    backups: usize,
}

impl RotatingWriter {
    pub fn new(path: PathBuf, max_bytes: u64, backups: usize) -> io::Result<Self> {
        let file = open(&path)?;
        let length = file.metadata()?.len();
        Ok(RotatingWriter {
            inner: Arc::new(Mutex::new(Inner {
                path,
                file,
                length,
                max_bytes,
                backups,
            })),
        })
    }

    fn write_all(&self, buffer: &[u8]) -> io::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if inner.length > 0 && inner.length + buffer.len() as u64 > inner.max_bytes {
            inner.rotate()?;
        }
        inner.file.write_all(buffer)?;
        inner.length += buffer.len() as u64;
        Ok(())
    }

    fn flush(&self) -> io::Result<()> {
        self.inner.lock().unwrap().file.flush()
    }
}

fn open(path: &std::path::Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

impl Inner {
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

        let _ = fs::remove_file(backup(self.backups));
        for number in (1..self.backups).rev() {
            let _ = fs::rename(backup(number), backup(number + 1));
        }
        let _ = fs::rename(&self.path, backup(1));

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
    let writer = match RotatingWriter::new(paths.log(), MAX_BYTES, BACKUPS) {
        Ok(writer) => writer,
        Err(error) => {
            eprintln!("{}: {error}", paths.log().display());
            return;
        }
    };

    let filter = LevelFilter::from_str(level).unwrap_or(LevelFilter::INFO);

    let _ = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_max_level(filter)
        .with_ansi(false)
        .try_init();
}
