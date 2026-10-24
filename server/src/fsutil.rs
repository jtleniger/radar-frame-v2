//! Durable artifact writes: every file is written to a temporary sibling,
//! synced, and renamed into place. A reader therefore sees either the previous
//! complete artifact or the new complete artifact, never a truncated one.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A temporary file in the destination's directory. Dropping it without
/// committing removes the temporary file.
pub struct TempPath {
    destination: PathBuf,
    temp: PathBuf,
    committed: bool,
}

impl TempPath {
    pub fn new(destination: &Path) -> io::Result<Self> {
        let name = destination.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{}: no file name", destination.display()),
            )
        })?;
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = destination.with_file_name(format!(
            ".{}.{}.{sequence}.tmp",
            name.to_string_lossy(),
            std::process::id()
        ));

        Ok(TempPath {
            destination: destination.to_path_buf(),
            temp,
            committed: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.temp
    }

    /// Rename the temporary file over the destination.
    pub fn commit(mut self) -> io::Result<()> {
        fs::rename(&self.temp, &self.destination)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.temp);
        }
    }
}

/// Write `bytes` to `path` atomically, syncing before the rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = TempPath::new(path)?;
    {
        let mut file = File::create(temp.path())?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    temp.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_the_destination_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");
        fs::write(&path, b"old").unwrap();

        atomic_write(&path, b"new").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new");
        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("artifact")]);
    }

    #[test]
    fn dropping_an_uncommitted_temp_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("artifact");
        let temp = TempPath::new(&destination).unwrap();
        fs::write(temp.path(), b"partial").unwrap();

        drop(temp);

        assert!(!destination.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
