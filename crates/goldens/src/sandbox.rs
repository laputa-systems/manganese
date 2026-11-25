use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;

#[cfg(not(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))))]
compile_error!("L0 isolation requires Linux on x86_64 or aarch64");

unsafe extern "C" {
    fn syscall(number: i64, ...) -> i64;
    fn prctl(option: i32, ...) -> i32;
    fn flock(fd: i32, operation: i32) -> i32;
}

#[repr(C)]
struct RulesetAttr { handled_access_fs: u64 }
#[repr(C)]
struct PathBeneathAttr { allowed_access: u64, parent_fd: i32, reserved: u32 }

pub fn lock_ratchet(id: &str) -> io::Result<File> {
    std::fs::create_dir_all(".work/tmp")?;
    let file = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(format!(".work/tmp/ratchet-{id}.lock"))?;
    // SAFETY: the descriptor belongs to the open file and LOCK_EX holds until it is closed.
    if unsafe { flock(file.as_raw_fd(), 2) } != 0 { return Err(io::Error::last_os_error()); }
    Ok(file)
}

pub fn restrict(executables: &[&Path], store: &Path) -> io::Result<()> {
    const EXECUTE: u64 = 1;
    const READ_FILE: u64 = 4;
    const READ_DIR: u64 = 8;
    let store = store.canonicalize()?;
    let mut paths = Vec::new();
    for path in ["/lib", "/lib64", "/usr/lib", "/usr/lib64"] {
        if let Ok(p) = Path::new(path).canonicalize() {
            if store.starts_with(&p) || p.starts_with(&store) {
                return Err(io::Error::other("store overlaps a runtime library directory"));
            }
            if !paths.contains(&p) { paths.push(p); }
        }
    }
    let attr = RulesetAttr { handled_access_fs: EXECUTE | READ_FILE | READ_DIR };
    // SAFETY: the pointer refers to a live ruleset attribute of the stated size.
    let fd = unsafe { syscall(444, &attr as *const RulesetAttr, std::mem::size_of::<RulesetAttr>(), 0u32) };
    if fd < 0 { return Err(io::Error::last_os_error()); }
    // SAFETY: a successful landlock_create_ruleset returns a new owned descriptor.
    let ruleset = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    for path in paths.iter().map(|p| (p.as_path(), EXECUTE | READ_FILE | READ_DIR)).chain(executables.iter().map(|p| (*p, EXECUTE | READ_FILE))) {
        let canonical = path.0.canonicalize()?;
        if canonical.starts_with(&store) { return Err(io::Error::other("executable is in the store")); }
        let file = File::open(canonical)?;
        let rule = PathBeneathAttr { allowed_access: path.1, parent_fd: file.as_raw_fd(), reserved: 0 };
        // SAFETY: the ruleset and path descriptors are live and the rule points to initialized memory.
        if unsafe { syscall(445, ruleset.as_raw_fd(), 1u32, &rule as *const PathBeneathAttr, 0u32) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    // SAFETY: PR_SET_NO_NEW_PRIVS accepts these integer arguments and affects only this process and its children.
    if unsafe { prctl(38, 1usize, 0usize, 0usize, 0usize) } != 0 { return Err(io::Error::last_os_error()); }
    // SAFETY: the ruleset descriptor is valid and flags are zero.
    if unsafe { syscall(446, ruleset.as_raw_fd(), 0u32) } < 0 { return Err(io::Error::last_os_error()); }
    Ok(())
}
