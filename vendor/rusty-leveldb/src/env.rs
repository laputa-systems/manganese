//! An `env` is an abstraction layer that allows the database to run both on different platforms as
//! well as persisting data on disk or in memory.

use crate::error::Result;

use std::fs::File;
use std::io::prelude::*;
use std::io::BufWriter;
#[cfg(unix)]
use std::os::unix::fs::FileExt;
#[cfg(windows)]
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};

pub trait RandomAccess {
    fn read_at(&self, off: usize, dst: &mut [u8]) -> Result<usize>;
}

#[cfg(unix)]
impl RandomAccess for File {
    fn read_at(&self, off: usize, dst: &mut [u8]) -> Result<usize> {
        Ok((self as &dyn FileExt).read_at(dst, off as u64)?)
    }
}

#[cfg(windows)]
impl RandomAccess for File {
    fn read_at(&self, off: usize, dst: &mut [u8]) -> Result<usize> {
        Ok((self as &dyn FileExt).seek_read(dst, off as u64)?)
    }
}

pub struct FileLock {
    pub id: String,
}

/// A writable file whose sync operation persists earlier writes or returns an error.
pub trait WritableFile: Write {
    fn sync(&mut self) -> Result<()>;
}

impl WritableFile for File {
    fn sync(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: the descriptor belongs to this open file and F_FULLFSYNC takes no argument.
            if unsafe { libc::fcntl(self.as_raw_fd(), libc::F_FULLFSYNC) } == 0 {
                return Ok(());
            }
        }
        self.sync_all()?;
        Ok(())
    }
}

impl<T: WritableFile + ?Sized> WritableFile for Box<T> {
    fn sync(&mut self) -> Result<()> {
        (**self).sync()
    }
}

impl<T: WritableFile> WritableFile for BufWriter<T> {
    fn sync(&mut self) -> Result<()> {
        self.flush()?;
        self.get_mut().sync()
    }
}

pub trait Env {
    fn open_sequential_file(&self, _: &Path) -> Result<Box<dyn Read>>;
    fn open_random_access_file(&self, _: &Path) -> Result<Box<dyn RandomAccess>>;
    fn open_writable_file(&self, _: &Path) -> Result<Box<dyn WritableFile>>;
    fn open_appendable_file(&self, _: &Path) -> Result<Box<dyn WritableFile>>;

    fn sync_dir(&self, _: &Path) -> Result<()>;

    fn exists(&self, _: &Path) -> Result<bool>;
    fn children(&self, _: &Path) -> Result<Vec<PathBuf>>;
    fn size_of(&self, _: &Path) -> Result<usize>;

    fn delete(&self, _: &Path) -> Result<()>;
    fn mkdir(&self, _: &Path) -> Result<()>;
    fn rmdir(&self, _: &Path) -> Result<()>;
    fn rename(&self, _: &Path, _: &Path) -> Result<()>;

    fn lock(&self, _: &Path) -> Result<FileLock>;
    fn unlock(&self, l: FileLock) -> Result<()>;

    fn new_logger(&self, _: &Path) -> Result<Logger>;

    fn micros(&self) -> u64;
    fn sleep_for(&self, micros: u32);
}

pub struct Logger {
    dst: Box<dyn Write>,
}

impl Logger {
    pub fn new(w: Box<dyn Write>) -> Logger {
        Logger { dst: w }
    }

    pub fn log(&mut self, message: &str) {
        let _ = self.dst.write(message.as_bytes());
        let _ = self.dst.write("\n".as_bytes());
    }
}

pub fn path_to_string(p: &Path) -> String {
    p.to_str().map(String::from).unwrap()
}

pub fn path_to_str(p: &Path) -> &str {
    p.to_str().unwrap()
}
