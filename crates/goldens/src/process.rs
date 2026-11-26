use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const STDERR_KEPT: usize = 16 * 1024;
const SIGKILL: i32 = 9;

unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

/// Kill the adapter's whole process group, so a helper it started cannot hold its pipes open.
pub(crate) fn kill_group(child: &Child) {
    let Ok(pid) = i32::try_from(child.id()) else { return };
    // SAFETY: kill takes plain integers; a negative pid names the process group the adapter leads.
    // An already-empty group fails with ESRCH, which leaves nothing to clean up.
    unsafe { kill(-pid, SIGKILL) };
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub deadline: Duration,
    pub max_output: usize,
}

impl Limits {
    pub const DEFAULT_DEADLINE_SECS: u64 = 300;
    pub const DEFAULT_MAX_OUTPUT: usize = 1 << 30;

    pub fn from_env() -> Result<Self, String> {
        let deadline = env_number("PEER_TIMEOUT_SECS", Self::DEFAULT_DEADLINE_SECS)?;
        let max_output = env_number("PEER_MAX_OUTPUT", Self::DEFAULT_MAX_OUTPUT as u64)?;
        Ok(Self { deadline: Duration::from_secs(deadline), max_output: usize::try_from(max_output).map_err(|_| "PEER_MAX_OUTPUT is too large")? })
    }
}

pub fn env_number(key: &str, default: u64) -> Result<u64, String> {
    match std::env::var(key) {
        Ok(s) if !s.is_empty() => parse_positive(key, &s),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(default),
        Err(e) => Err(format!("{key}: {e}")),
    }
}

pub fn parse_positive(key: &str, value: &str) -> Result<u64, String> {
    match value.parse::<u64>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(format!("{key} must be a positive integer, got {value:?}")),
    }
}

/// The private directory tree adapters may write and read back, such as a database an adapter opens
/// and reopens. Each adapter run gets an empty subdirectory named by GOLDENS_SCRATCH; the tree is
/// removed when the scratch is dropped.
pub struct Scratch {
    root: PathBuf,
    next: u64,
}

impl Scratch {
    pub fn create(label: &str) -> Result<Self, String> {
        let root = Path::new(".work/tmp/goldens-scratch").join(format!("{label}-{}", std::process::id()));
        match fs::remove_dir_all(&root) {
            Err(e) if e.kind() != ErrorKind::NotFound => return Err(format!("clear {}: {e}", root.display())),
            _ => {}
        }
        fs::create_dir_all(&root).map_err(|e| format!("create {}: {e}", root.display()))?;
        let root = root.canonicalize().map_err(|e| format!("resolve {}: {e}", root.display()))?;
        Ok(Self { root, next: 0 })
    }

    pub fn root(&self) -> &Path { &self.root }

    pub fn fresh(&mut self) -> Result<PathBuf, String> {
        self.next += 1;
        let dir = self.root.join(self.next.to_string());
        fs::create_dir(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        Ok(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Scratch holds only adapter working files; a tree left behind is disposable .work/tmp content.
        let _ = fs::remove_dir_all(&self.root);
    }
}

enum Frame {
    Record(Vec<u8>),
    End,
    Invalid(String),
}

/// An adapter speaking MGB1 over pipes, with every response bounded by a deadline and an output
/// budget. A peer that hangs, exits early or frames its output wrongly is reported, never waited on
/// forever. The peer owns its pipe threads: stopping it, on drop or failure, kills the adapter's
/// process group, which closes the pipes those threads block on, and joins them.
pub struct Peer {
    label: String,
    child: Child,
    reaped: bool,
    input: Option<Sender<Vec<u8>>>,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    frames: Receiver<Frame>,
    stderr: Option<JoinHandle<Vec<u8>>>,
    limits: Limits,
}

impl Peer {
    pub fn start(program: &Path, args: &[String], label: &str, limits: Limits, scratch: &Path) -> Result<Self, String> {
        let mut child = Command::new(program).args(args).env("GOLDENS_SCRATCH", scratch).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0).spawn()
            .map_err(|e| format!("{label} {}: {e}", program.display()))?;
        let (Some(mut stdin), Some(mut stdout), Some(mut stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
            kill_group(&child);
            let _ = child.wait();
            return Err(format!("{label}: missing pipes"));
        };
        let (input, records) = mpsc::channel::<Vec<u8>>();
        let writer = std::thread::spawn(move || {
            if stdin.write_all(b"MGB1").is_err() { return; }
            for record in records {
                let Ok(len) = u32::try_from(record.len()) else { return };
                if stdin.write_all(&len.to_be_bytes()).and_then(|_| stdin.write_all(&record)).and_then(|_| stdin.flush()).is_err() { return; }
            }
        });
        let (sender, frames) = mpsc::channel();
        let max_output = limits.max_output;
        let reader = std::thread::spawn(move || read_frames(&mut stdout, max_output, &sender));
        let stderr = std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buffer = [0; 8192];
            while let Ok(n) = stderr.read(&mut buffer) {
                if n == 0 { break; }
                kept.extend_from_slice(&buffer[..n.min(STDERR_KEPT.saturating_sub(kept.len()))]);
            }
            kept
        });
        Ok(Self { label: label.into(), child, reaped: false, input: Some(input), writer: Some(writer), reader: Some(reader), frames, stderr: Some(stderr), limits })
    }

    pub fn record(&mut self, record: &[u8]) -> Result<Vec<u8>, String> {
        let input = self.input.as_ref().ok_or_else(|| format!("{}: input closed", self.label))?;
        // A failed send means the writer met a closed pipe; the response path reports why.
        let _ = input.send(record.to_vec());
        self.response()
    }

    /// Send one record and close the input, as a one-shot adapter expects.
    pub fn exchange(mut self, record: &[u8]) -> Result<Vec<u8>, String> {
        let input = self.input.take().ok_or_else(|| format!("{}: input closed", self.label))?;
        // A failed send means the writer met a closed pipe; the response path reports why.
        let _ = input.send(record.to_vec());
        drop(input);
        let response = self.response()?;
        self.finish()?;
        Ok(response)
    }

    fn response(&mut self) -> Result<Vec<u8>, String> {
        match self.frames.recv_timeout(self.limits.deadline) {
            Ok(Frame::Record(record)) => Ok(record),
            Ok(Frame::End) | Err(RecvTimeoutError::Disconnected) => Err(self.failure("exited before responding")),
            Ok(Frame::Invalid(reason)) => Err(self.failure(&reason)),
            Err(RecvTimeoutError::Timeout) => Err(self.failure(&format!("did not respond within {}s", self.limits.deadline.as_secs_f64()))),
        }
    }

    pub fn finish(&mut self) -> Result<(), String> {
        drop(self.input.take());
        match self.frames.recv_timeout(self.limits.deadline) {
            Ok(Frame::End) | Err(RecvTimeoutError::Disconnected) => {}
            Ok(Frame::Record(_)) => return Err(self.failure("wrote an unrequested record")),
            Ok(Frame::Invalid(reason)) => return Err(self.failure(&reason)),
            Err(RecvTimeoutError::Timeout) => return Err(self.failure("did not exit after its input closed")),
        }
        match self.wait(Instant::now() + self.limits.deadline) {
            Some(status) if status.success() => Ok(()),
            Some(status) => Err(self.failure(&format!("exited {status}"))),
            None => Err(self.failure("did not exit after its input closed")),
        }
    }

    fn wait(&mut self, deadline: Instant) -> Option<ExitStatus> {
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    kill_group(&self.child);
                    self.reaped = true;
                    return Some(status);
                }
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                _ => return None,
            }
        }
    }

    fn failure(&mut self, reason: &str) -> String {
        drop(self.input.take());
        if !self.reaped {
            kill_group(&self.child);
            let _ = self.child.wait();
            self.reaped = true;
        }
        let stderr = self.stderr.take().and_then(|thread| thread.join().ok()).unwrap_or_default();
        self.stop();
        let stderr = String::from_utf8_lossy(&stderr);
        if stderr.trim().is_empty() { format!("{} {reason}", self.label) } else { format!("{} {reason}: {}", self.label, stderr.trim_end()) }
    }

    fn stop(&mut self) {
        drop(self.input.take());
        if !self.reaped {
            kill_group(&self.child);
            // The exit status of a killed adapter carries nothing the failure has not reported.
            let _ = self.child.wait();
            self.reaped = true;
        }
        for thread in [self.writer.take(), self.reader.take()].into_iter().flatten() {
            // A pipe thread that panicked has nothing left to report; joining only releases it.
            let _ = thread.join();
        }
        if let Some(stderr) = self.stderr.take() { let _ = stderr.join(); }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_frames(stdout: &mut impl Read, max_output: usize, sender: &Sender<Frame>) {
    let mut header = [0; 4];
    match read_full(stdout, &mut header) {
        Ok(0) => { let _ = sender.send(Frame::End); return; }
        Ok(4) if &header == b"MGB1" => {}
        Ok(_) => { let _ = sender.send(Frame::Invalid("wrote an invalid protocol header".into())); return; }
        Err(e) => { let _ = sender.send(Frame::Invalid(format!("read failed: {e}"))); return; }
    }
    loop {
        let mut length = [0; 4];
        match read_full(stdout, &mut length) {
            Ok(0) => { let _ = sender.send(Frame::End); return; }
            Ok(4) => {}
            Ok(_) => { let _ = sender.send(Frame::Invalid("wrote a truncated record length".into())); return; }
            Err(e) => { let _ = sender.send(Frame::Invalid(format!("read failed: {e}"))); return; }
        }
        let length = u32::from_be_bytes(length) as usize;
        if length > max_output {
            let _ = sender.send(Frame::Invalid(format!("announced a {length}-byte record, over the {max_output}-byte output budget")));
            return;
        }
        let mut record = vec![0; length];
        match read_full(stdout, &mut record) {
            Ok(n) if n == length => { if sender.send(Frame::Record(record)).is_err() { return; } }
            Ok(_) => { let _ = sender.send(Frame::Invalid("wrote a truncated record".into())); return; }
            Err(e) => { let _ = sender.send(Frame::Invalid(format!("read failed: {e}"))); return; }
        }
    }
}

fn read_full(reader: &mut impl Read, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn limits() -> Limits { Limits { deadline: Duration::from_millis(500), max_output: 64 } }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("goldens-process-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_echoing_peer_answers_and_exits_cleanly() {
        let dir = scratch("echo");
        let peer = script(&dir, "echo", "exec cat");
        let mut session = Peer::start(&peer, &[], "echo", limits(), &dir).unwrap();
        assert_eq!(session.record(b"abc").unwrap(), b"abc");
        assert_eq!(session.record(b"").unwrap(), b"");
        session.finish().unwrap();
        assert_eq!(Peer::start(&peer, &[], "echo", limits(), &dir).unwrap().exchange(b"one").unwrap(), b"one");
    }

    #[test]
    fn a_hanging_peer_times_out() {
        let dir = scratch("hang");
        let peer = script(&dir, "hang", "exec sleep 30");
        let started = Instant::now();
        let error = Peer::start(&peer, &[], "hang", limits(), &dir).unwrap().exchange(b"x").unwrap_err();
        assert!(error.contains("did not respond within"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn an_early_exiting_peer_is_reported_with_its_stderr() {
        let dir = scratch("exit");
        let peer = script(&dir, "exit", "echo broken >&2; exit 3");
        let error = Peer::start(&peer, &[], "exit", limits(), &dir).unwrap().exchange(b"x").unwrap_err();
        assert!(error.contains("exited before responding") && error.contains("broken"), "{error}");
    }

    #[test]
    fn malformed_framing_and_oversized_records_are_rejected() {
        let dir = scratch("framing");
        for (name, body, expected) in [
            ("header", "printf XXXX", "invalid protocol header"),
            ("length", "printf MGB1; printf '\\000\\000'", "truncated record length"),
            ("record", "printf MGB1; printf '\\000\\000\\000\\010abc'", "truncated record"),
            ("budget", "printf MGB1; printf '\\000\\001\\000\\000'", "output budget"),
        ] {
            let peer = script(&dir, name, &format!("cat >/dev/null & {body}; wait"));
            let error = Peer::start(&peer, &[], name, limits(), &dir).unwrap().exchange(b"x").unwrap_err();
            assert!(error.contains(expected), "{name}: {error}");
        }
    }

    #[test]
    fn a_peer_writing_an_unrequested_record_fails_its_finish() {
        let dir = scratch("extra");
        let peer = script(&dir, "extra", "cat >/dev/null; printf 'MGB1\\000\\000\\000\\001a\\000\\000\\000\\001b'");
        let error = Peer::start(&peer, &[], "extra", limits(), &dir).unwrap().exchange(b"x").unwrap_err();
        assert!(error.contains("unrequested record"), "{error}");
    }

    #[test]
    fn an_adapter_writes_and_reads_back_its_scratch() {
        let dir = scratch("scratch");
        let peer = script(&dir, "scratch", "printf state > \"$GOLDENS_SCRATCH/db\"; cat \"$GOLDENS_SCRATCH/db\" >&2; exec cat");
        let work = dir.join("work");
        std::fs::create_dir(&work).unwrap();
        assert_eq!(Peer::start(&peer, &[], "scratch", limits(), &work).unwrap().exchange(b"x").unwrap(), b"x");
        assert_eq!(std::fs::read(work.join("db")).unwrap(), b"state");
    }

    #[test]
    fn case_budgets_must_be_positive() {
        assert_eq!(parse_positive("CASES", "12"), Ok(12));
        for value in ["0", "-1", "", "1.5", "ten"] {
            assert!(parse_positive("CASES", value).is_err(), "{value}");
        }
    }
}
