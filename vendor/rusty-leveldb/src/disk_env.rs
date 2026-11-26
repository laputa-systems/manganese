use crate::env::{path_to_str, Env, FileLock, Logger, RandomAccess, WritableFile};
use crate::env_common::{micros, sleep_for};
use crate::error::{err, Result, Status, StatusCode};
use crate::types::{share, Shared};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, ErrorKind, Read};
use std::iter::FromIterator;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

type FileDescriptor = i32;

#[derive(Clone)]
pub struct PosixDiskEnv {
    locks: Shared<HashMap<String, DiskLock>>,
}

#[cfg(unix)]
type LockIdentity = (u64, u64);
#[cfg(not(unix))]
type LockIdentity = PathBuf;

static PROCESS_LOCKS: OnceLock<Mutex<HashMap<LockIdentity, Vec<File>>>> = OnceLock::new();

fn process_locks() -> &'static Mutex<HashMap<LockIdentity, Vec<File>>> {
    PROCESS_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(unix)]
fn lock_identity(_path: &Path, metadata: &fs::Metadata) -> io::Result<LockIdentity> {
    use std::os::unix::fs::MetadataExt;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn lock_identity(path: &Path, _metadata: &fs::Metadata) -> io::Result<LockIdentity> {
    path.canonicalize()
}

#[cfg(unix)]
fn set_file_lock(file: &File, lock: bool) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: a zeroed flock is initialized below before it is passed to fcntl.
    let mut record: libc::flock = unsafe { std::mem::zeroed() };
    record.l_type = if lock { libc::F_WRLCK } else { libc::F_UNLCK } as _;
    record.l_whence = libc::SEEK_SET as _;
    loop {
        // SAFETY: the file descriptor and flock pointer remain valid for the call.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &record) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() == ErrorKind::Interrupted {
            continue;
        }
        if lock && error.raw_os_error() == Some(libc::EACCES) {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        return Err(error);
    }
}

#[cfg(not(unix))]
fn set_file_lock(file: &File, lock: bool) -> io::Result<()> {
    if lock { file.try_lock().map_err(io::Error::from) } else { file.unlock() }
}

struct DiskLock(Option<LockIdentity>);

impl DiskLock {
    fn release(&mut self) -> io::Result<()> {
        let Some(identity) = self.0.take() else { return Ok(()); };
        let mut locks = process_locks().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(files) = locks.remove(&identity) {
            let result = set_file_lock(&files[0], false);
            drop(files);
            result
        } else {
            Err(io::Error::new(ErrorKind::NotFound, "lock is not held"))
        }
    }
}

impl Drop for DiskLock {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

impl Default for PosixDiskEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl PosixDiskEnv {
    pub fn new() -> PosixDiskEnv {
        PosixDiskEnv {
            locks: share(HashMap::new()),
        }
    }
}

/// map_err_with_name annotates an io::Error with information about the operation and the file.
fn map_err_with_name(method: &'static str, f: &Path, e: io::Error) -> Status {
    let mut s = Status::from(e);
    s.err = format!("{}: {}: {}", method, s.err, path_to_str(f));
    s
}

fn map_lock_error(p: &Path, error: io::Error) -> Status {
    if error.kind() == ErrorKind::WouldBlock {
        Status::new(StatusCode::LockError, "lock on database is already held by different process")
    } else {
        map_err_with_name("lock", p, error)
    }
}

// Note: We're using Ok(f()?) in several locations below in order to benefit from the automatic
// error conversion using std::convert::From.
impl Env for PosixDiskEnv {
    fn open_sequential_file(&self, p: &Path) -> Result<Box<dyn Read>> {
        Ok(Box::new(
            fs::OpenOptions::new()
                .read(true)
                .open(p)
                .map_err(|e| map_err_with_name("open (seq)", p, e))?,
        ))
    }
    fn open_random_access_file(&self, p: &Path) -> Result<Box<dyn RandomAccess>> {
        fs::OpenOptions::new()
            .read(true)
            .open(p)
            .map(|f| {
                let b: Box<dyn RandomAccess> = Box::new(f);
                b
            })
            .map_err(|e| map_err_with_name("open (randomaccess)", p, e))
    }
    fn open_writable_file(&self, p: &Path) -> Result<Box<dyn WritableFile>> {
        Ok(Box::new(
            fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .append(false)
                .open(p)
                .map_err(|e| map_err_with_name("open (write)", p, e))?,
        ))
    }
    fn open_appendable_file(&self, p: &Path) -> Result<Box<dyn WritableFile>> {
        Ok(Box::new(
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .map_err(|e| map_err_with_name("open (append)", p, e))?,
        ))
    }

    fn sync_dir(&self, p: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            let mut dir = File::open(p).map_err(|e| map_err_with_name("open (directory)", p, e))?;
            dir.sync()?;
        }
        #[cfg(not(unix))]
        let _ = p;
        Ok(())
    }

    fn exists(&self, p: &Path) -> Result<bool> {
        Ok(p.exists())
    }
    fn children(&self, p: &Path) -> Result<Vec<PathBuf>> {
        let dir_reader = fs::read_dir(p).map_err(|e| map_err_with_name("children", p, e))?;
        let filenames = dir_reader
            .map(|r| match r {
                Ok(_) => {
                    let direntry = r.unwrap();
                    Path::new(&direntry.file_name()).to_owned()
                }
                Err(_) => Path::new("").to_owned(),
            })
            .filter(|s| !s.as_os_str().is_empty());
        Ok(Vec::from_iter(filenames))
    }
    fn size_of(&self, p: &Path) -> Result<usize> {
        let meta = fs::metadata(p).map_err(|e| map_err_with_name("size_of", p, e))?;
        Ok(meta.len() as usize)
    }

    fn delete(&self, p: &Path) -> Result<()> {
        fs::remove_file(p).map_err(|e| map_err_with_name("delete", p, e))
    }
    fn mkdir(&self, p: &Path) -> Result<()> {
        fs::create_dir_all(p).map_err(|e| map_err_with_name("mkdir", p, e))
    }
    fn rmdir(&self, p: &Path) -> Result<()> {
        fs::remove_dir_all(p).map_err(|e| map_err_with_name("rmdir", p, e))
    }
    fn rename(&self, old: &Path, new: &Path) -> Result<()> {
        fs::rename(old, new).map_err(|e| map_err_with_name("rename", old, e))
    }

    fn lock(&self, p: &Path) -> Result<FileLock> {
        let mut locks = self.locks.borrow_mut();

        if let std::collections::hash_map::Entry::Vacant(e) =
            locks.entry(p.to_str().unwrap().to_string())
        {
            let mut process = process_locks().lock().unwrap_or_else(|e| e.into_inner());
            if let Ok(metadata) = fs::metadata(p) {
                let identity = lock_identity(p, &metadata).map_err(|error| map_lock_error(p, error))?;
                if process.contains_key(&identity) {
                    return err(StatusCode::AlreadyExists, "Lock is held");
                }
            }
            let f = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(p)
                .map_err(|e| map_err_with_name("lock", p, e))?;

            let identity = lock_identity(p, &f.metadata().map_err(|error| map_lock_error(p, error))?)
                .map_err(|error| map_lock_error(p, error))?;
            if let Some(files) = process.get_mut(&identity) {
                files.push(f);
                return err(StatusCode::AlreadyExists, "Lock is held");
            }
            set_file_lock(&f, true).map_err(|error| map_lock_error(p, error))?;
            process.insert(identity.clone(), vec![f]);
            e.insert(DiskLock(Some(identity)));
            let lock = FileLock {
                id: p.to_str().unwrap().to_string(),
            };
            Ok(lock)
        } else {
            Err(Status::new(StatusCode::AlreadyExists, "Lock is held"))
        }
    }
    fn unlock(&self, l: FileLock) -> Result<()> {
        let mut locks = self.locks.borrow_mut();
        if !locks.contains_key(&l.id) {
            err(
                StatusCode::LockError,
                &format!("unlocking a file that is not locked: {}", l.id),
            )
        } else {
            let mut lock = locks.remove(&l.id).unwrap();
            lock.release().map_err(|error| map_lock_error(Path::new(&l.id), error))
        }
    }

    fn new_logger(&self, p: &Path) -> Result<Logger> {
        self.open_appendable_file(p)
            .map(|dst| Logger::new(Box::new(dst)))
    }

    fn micros(&self) -> u64 {
        micros()
    }

    fn sleep_for(&self, micros: u32) {
        sleep_for(micros);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::convert::AsRef;
    use std::io::Write;
    use std::iter::FromIterator;

    #[test]
    fn lock_error_preserves_the_io_error() {
        let path = Path::new("db/LOCK");
        let denied = map_lock_error(path, io::Error::new(ErrorKind::PermissionDenied, "access denied"));
        assert_eq!(denied.code, StatusCode::PermissionDenied);
        assert!(denied.err.contains("access denied"));
        assert!(denied.err.contains("db/LOCK"));
        let held = map_lock_error(path, io::Error::new(ErrorKind::WouldBlock, "busy"));
        assert_eq!(held.code, StatusCode::LockError);
    }

    #[cfg(unix)]
    #[test]
    fn database_lock_excludes_posix_record_lock_in_another_process() {
        use std::os::fd::AsRawFd;
        use std::process::Command;

        if let Some(path) = std::env::var_os("RUSTY_LEVELDB_LOCK_PROBE") {
            let file = fs::OpenOptions::new().write(true).open(path).unwrap();
            // SAFETY: a zeroed flock is initialized below before it is passed to fcntl.
            let mut lock: libc::flock = unsafe { std::mem::zeroed() };
            lock.l_type = libc::F_WRLCK as _;
            lock.l_whence = libc::SEEK_SET as _;
            // SAFETY: the file descriptor and flock pointer remain valid for the call.
            let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock) };
            assert_eq!(result, -1, "another process acquired the database lock");
            let error = io::Error::last_os_error();
            assert!(error.kind() == ErrorKind::WouldBlock || error.raw_os_error() == Some(libc::EACCES));
            return;
        }

        let path = std::env::temp_dir().join(format!("rusty-leveldb-lock-{}", std::process::id()));
        let env = PosixDiskEnv::new();
        let lock = env.lock(&path).unwrap();
        let other = PosixDiskEnv::new();
        assert!(other.lock(&path).is_err());
        let status = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("disk_env::tests::database_lock_excludes_posix_record_lock_in_another_process")
            .env("RUSTY_LEVELDB_LOCK_PROBE", &path)
            .status().unwrap();
        env.unlock(lock).unwrap();
        let other_lock = other.lock(&path).unwrap();
        other.unlock(other_lock).unwrap();
        let abandoned = PosixDiskEnv::new();
        let _abandoned_lock = abandoned.lock(&path).unwrap();
        drop(abandoned);
        let reacquired = env.lock(&path).unwrap();
        env.unlock(reacquired).unwrap();
        fs::remove_file(path).unwrap();
        assert!(status.success());
    }

    #[cfg(unix)]
    #[test]
    fn database_lock_respects_an_existing_posix_record_lock() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::fd::AsRawFd;
        use std::process::{Command, Stdio};

        if let Some(path) = std::env::var_os("RUSTY_LEVELDB_LOCK_HOLDER") {
            let file = fs::OpenOptions::new().write(true).open(path).unwrap();
            // SAFETY: a zeroed flock is initialized below before it is passed to fcntl.
            let mut lock: libc::flock = unsafe { std::mem::zeroed() };
            lock.l_type = libc::F_WRLCK as _;
            lock.l_whence = libc::SEEK_SET as _;
            // SAFETY: the file descriptor and flock pointer remain valid for the call.
            assert_eq!(unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock) }, 0);
            println!("LOCK_HELD");
            io::stdout().flush().unwrap();
            let mut line = String::new();
            io::stdin().read_line(&mut line).unwrap();
            return;
        }

        let path = std::env::temp_dir().join(format!("rusty-leveldb-held-{}", std::process::id()));
        fs::write(&path, b"").unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("disk_env::tests::database_lock_respects_an_existing_posix_record_lock")
            .arg("--nocapture")
            .env("RUSTY_LEVELDB_LOCK_HOLDER", &path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut ready = false;
        for line in output.by_ref().lines() {
            if line.unwrap().contains("LOCK_HELD") {
                ready = true;
                break;
            }
        }
        assert!(ready, "record-lock holder failed to start");
        let env = PosixDiskEnv::new();
        let result = env.lock(&path);
        child.stdin.take().unwrap().write_all(b"release\n").unwrap();
        let status = child.wait().unwrap();
        let lock = env.lock(&path).unwrap();
        env.unlock(lock).unwrap();
        fs::remove_file(path).unwrap();
        assert!(result.is_err());
        assert!(status.success());
    }

    #[test]
    fn test_files() {
        let n = "testfile.xyz".to_string();
        let name = n.as_ref();
        let env = PosixDiskEnv::new();

        // exists, size_of, delete
        assert!(env.open_appendable_file(name).is_ok());
        assert!(env.exists(name).unwrap_or(false));
        assert_eq!(env.size_of(name).unwrap_or(1), 0);
        assert!(env.delete(name).is_ok());

        assert!(env.open_writable_file(name).is_ok());
        assert!(env.exists(name).unwrap_or(false));
        assert_eq!(env.size_of(name).unwrap_or(1), 0);
        assert!(env.delete(name).is_ok());

        {
            // write
            let mut f = env.open_writable_file(name).unwrap();
            let _ = f.write("123xyz".as_bytes());
            assert_eq!(6, env.size_of(name).unwrap_or(0));

            // rename
            let newname = Path::new("testfile2.xyz");
            assert!(env.rename(name, newname).is_ok());
            assert_eq!(6, env.size_of(newname).unwrap());
            assert!(!env.exists(name).unwrap());
            // rename back so that the remaining tests can use the file.
            assert!(env.rename(newname, name).is_ok());
        }

        assert!(env.open_sequential_file(name).is_ok());
        assert!(env.open_random_access_file(name).is_ok());

        assert!(env.delete(name).is_ok());
    }

    #[test]
    fn test_locking() {
        let env = PosixDiskEnv::new();
        let n = "testfile.123".to_string();
        let name = n.as_ref();

        {
            let mut f = env.open_writable_file(name).unwrap();
            let _ = f.write("123xyz".as_bytes());
            assert_eq!(env.size_of(name).unwrap_or(0), 6);
        }

        {
            let r = env.lock(name);
            assert!(r.is_ok());
            env.unlock(r.unwrap()).unwrap();
        }

        {
            let r = env.lock(name);
            assert!(r.is_ok());
            let s = env.lock(name);
            assert!(s.is_err());
            env.unlock(r.unwrap()).unwrap();
        }

        assert!(env.delete(name).is_ok());
    }

    #[test]
    fn test_dirs() {
        let d = "subdir/";
        let dirname = d.as_ref();
        let env = PosixDiskEnv::new();

        assert!(env.mkdir(dirname).is_ok());
        assert!(env
            .open_writable_file(
                String::from_iter(vec![d.to_string(), "f1.txt".to_string()].into_iter()).as_ref()
            )
            .is_ok());
        assert_eq!(env.children(dirname).unwrap().len(), 1);
        assert!(env.rmdir(dirname).is_ok());
    }
}
