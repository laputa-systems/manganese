use goldens::{Lane, RunOptions, Spec, sha256};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn default_store() -> Result<PathBuf, String> {
    if let Some(store) = std::env::var_os("MANGANESE_STORE") { return Ok(store.into()); }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/share/manganese/store"))
}

fn run_args(args: Vec<OsString>) -> Result<(), String> {
    let mut args = args.into_iter();
    let id = args.next().ok_or("missing box id")?.into_string().map_err(|_| "non-UTF8 box id")?;
    let lane = match args.next().and_then(|s| s.into_string().ok()).as_deref() {
        Some("seen") => Lane::Seen,
        Some("holdout") => Lane::Holdout,
        _ => return Err("lane must be seen or holdout".into()),
    };
    let mut options = RunOptions { box_id: id, lane, store: default_store()?, oracle_override: None, candidate_override: None, oracle_args: Vec::new(), ratchet: false, probe_holdout: None };
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--oracle") => options.oracle_override = Some(args.next().ok_or("missing oracle path")?.into()),
            Some("--candidate") => options.candidate_override = Some(args.next().ok_or("missing candidate path")?.into()),
            Some("--oracle-arg") => options.oracle_args.push(args.next().ok_or("missing oracle argument")?.into_string().map_err(|_| "non-UTF8 oracle argument")?),
            Some("--ratchet") => options.ratchet = true,
            Some("--probe-holdout") => options.probe_holdout = Some(args.next().ok_or("missing holdout path")?.into()),
            _ => return Err(format!("unknown option: {arg:?}")),
        }
    }
    goldens::run(&options)
}

fn child(store: &Path, args: &[&str], marker: &Path) -> Result<std::process::Output, String> {
    child_in(Path::new("."), store, args, marker)
}

fn child_in(dir: &Path, store: &Path, args: &[&str], marker: &Path) -> Result<std::process::Output, String> {
    Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("run").args(args).env("MANGANESE_STORE", store).env("MANGANESE_MARKER", marker)
        .current_dir(dir)
        .output().map_err(|e| e.to_string())
}

fn require(output: &std::process::Output, success: bool, needle: &str) -> Result<(), String> {
    let err = String::from_utf8_lossy(&output.stderr);
    if output.status.success() != success || !err.contains(needle) {
        return Err(format!("status {}; stderr: {err}", output.status));
    }
    Ok(())
}

fn selftest() -> Result<(), String> {
    let root = Path::new(".work/tmp").canonicalize().map_err(|e| e.to_string())?;
    let temp = root.join(format!("goldens-selftest-{}", std::process::id()));
    fs::create_dir(&temp).map_err(|e| e.to_string())?;
    let store = temp.join("store");
    fs::create_dir_all(store.join("sha256")).map_err(|e| e.to_string())?;
    for word in ["alpha", "beta", "gamma", "delta", "omega"] {
        for value in [word.to_owned(), word.to_ascii_uppercase()] {
            fs::write(store.join("sha256").join(sha256(value.as_bytes())), value).map_err(|e| e.to_string())?;
        }
    }
    let marker = temp.join("candidate-ran");
    let wrong = ".work/tmp/target/x86_64-unknown-linux-musl/debug/wrong";
    let marker_bin = ".work/tmp/target/x86_64-unknown-linux-musl/debug/marker";
    let executable = |path: &str| -> Result<String, String> {
        Path::new(path).canonicalize().map_err(|e| e.to_string())?.into_os_string().into_string().map_err(|_| "non-UTF8 executable path".into())
    };
    let oracle = executable(".work/tmp/target-oracle/x86_64-unknown-linux-musl/debug/selftest-oracle")?;
    let candidate = executable(".work/tmp/target/x86_64-unknown-linux-musl/debug/candidate")?;
    let wrong_absolute = executable(wrong)?;
    let fuzz_wrong = executable(".work/tmp/target/x86_64-unknown-linux-musl/debug/fuzz-wrong")?;
    let partial = executable(".work/tmp/target/x86_64-unknown-linux-musl/debug/partial")?;
    let fixture = temp.join("fixture");
    fs::create_dir_all(fixture.join("boxes/selftest")).map_err(|e| e.to_string())?;
    fs::copy("goldens.lock", fixture.join("goldens.lock")).map_err(|e| e.to_string())?;
    fs::copy("boxes/selftest/box.toml", fixture.join("boxes/selftest/box.toml")).map_err(|e| e.to_string())?;
    let score = fixture.join("boxes/selftest/scoreboard.toml");
    let mut failed = false;
    let mut case = |name: &str, result: Result<(), String>| {
        match result {
            Ok(()) => println!("ok   {name}"),
            Err(e) => { println!("FAIL {name}"); eprintln!("{name}: {e}"); failed = true; }
        }
    };
    case("positive exact match", (|| {
        require(&child(&store, &["selftest", "seen"], &marker)?, true, "")?;
        require(&child(&store, &["selftest", "holdout"], &marker)?, true, "")?;
        for value in ["alpha", "ALPHA"] {
            let path = store.join("sha256").join(sha256(value.as_bytes()));
            fs::write(&path, b"wrong").map_err(|e| e.to_string())?;
            let result = child(&store, &["selftest", "seen"], &marker);
            fs::write(&path, value).map_err(|e| e.to_string())?;
            require(&result?, false, "digest or size mismatch")?;
        }
        Ok(())
    })());
    case("negative control (wrong crate) fails", (|| {
        fs::write(&score, "version = 1\npassing = []\n").map_err(|e| e.to_string())?;
        let args = ["selftest", "seen", "--oracle", &oracle, "--candidate", &wrong_absolute];
        require(&child_in(&fixture, &store, &args, &marker)?, false, "golden mismatch")?;
        if fs::read_to_string(&score).map_err(|e| e.to_string())? != "version = 1\npassing = []\n" { return Err("ordinary run changed scoreboard".into()); }
        Ok(())
    })());
    case("scoreboard regression fails", (|| {
        let partial_run = ["selftest", "seen", "--oracle", &oracle, "--candidate", &partial, "--ratchet"];
        require(&child_in(&fixture, &store, &partial_run, &marker)?, false, "golden mismatch: beta")?;
        let updated = fs::read_to_string(&score).map_err(|e| e.to_string())?;
        if updated != "version = 1\npassing = [\"alpha\", \"gamma\"]\n" { return Err(format!("unexpected ratchet: {updated}")); }
        let regression = ["selftest", "seen", "--oracle", &oracle, "--candidate", &wrong_absolute];
        require(&child_in(&fixture, &store, &regression, &marker)?, false, "scoreboard regression: alpha, gamma")?;
        let passing = ["selftest", "seen", "--oracle", &oracle, "--candidate", &candidate];
        require(&child_in(&fixture, &store, &passing, &marker)?, true, "")?;
        if fs::read_to_string(&score).map_err(|e| e.to_string())? != updated { return Err("ordinary improvement changed scoreboard".into()); }
        let ratchet = ["selftest", "seen", "--oracle", &oracle, "--candidate", &candidate, "--ratchet"];
        require(&child_in(&fixture, &store, &ratchet, &marker)?, true, "")?;
        if fs::read_to_string(&score).map_err(|e| e.to_string())? != "version = 1\npassing = [\"alpha\", \"beta\", \"gamma\"]\n" { return Err("ratchet did not record improvements".into()); }
        Ok(())
    })());
    case("holdout unreadable from L0", (|| {
        for value in ["delta", "DELTA", "omega", "OMEGA"] {
            let path = store.join("sha256").join(sha256(value.as_bytes()));
            let path = path.to_str().ok_or("non-UTF8 test path")?;
            require(&child(&store, &["selftest", "seen", "--probe-holdout", path], &marker)?, true, "")?;
        }
        Ok(())
    })());
    case("box.toml missing each U15 declaration rejected", (|| {
        let original = fs::read_to_string("boxes/selftest/box.toml").map_err(|e| e.to_string())?;
        let keys = fs::read_to_string("boxes/selftest/missing-fields.txt").map_err(|e| e.to_string())?;
        for key in keys.lines() {
            let omitted = original.lines().filter(|line| line.split_once('=').map(|(k, _)| k.trim() != key).unwrap_or(true)).collect::<Vec<_>>().join("\n");
            match Spec::parse(&omitted) { Err(e) if e.contains(key) => (), Err(e) => return Err(format!("missing {key}: {e}")), Ok(_) => return Err(format!("missing {key} accepted")) }
        }
        for invalid in [
            original.replace("comparison = \"exact\"", "comparison = \"tolerance\""),
            original.replace("holdout_percent = 40", "holdout_percent = 29"),
            original.replace("wire_version = 1", "wire_version = \"1\""),
            original.replace("holdout_percent = 40", "holdout_percent = 040"),
            original.replace("anchor = \"none: the synthetic reference is not a browser\"", "anchor = \"none\""),
            original.replace("anchor = \"none: the synthetic reference is not a browser\"", "anchor = \"make selftest\""),
            original.replace("anchor = \"none: the synthetic reference is not a browser\"", "anchor = \"not established\""),
            original.replace("acceptance_cases = 1000", "acceptance_cases = 0"),
            original.replace("arm64_evidence = \"harness:", "arm64_evidence = \"invalid:"),
            original.replace("varying_relationships = \"none: every output byte is deterministic\"", "varying_relationships = \"timestamp varies\""),
            original.replace("improvements = \"none: no synthetic improvement is required\"", "improvements = \"improvement without an assertion\""),
        ] {
            if Spec::parse(&invalid).is_ok() { return Err("invalid declaration accepted".into()); }
        }
        Ok(())
    })());
    case("self-disagreeing oracle fails before candidate compare", (|| {
        let out = child(&store, &["selftest", "seen", "--candidate", marker_bin, "--oracle-arg", "--nondeterministic"], &marker)?;
        require(&out, false, "oracle self-disagreement: alpha")?;
        if marker.exists() { return Err("candidate ran before oracle agreement".into()); }
        Ok(())
    })());
    case("fuzz finds an injected divergence", (|| {
        let out = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
            .args(["fuzz", "selftest", "--candidate", &fuzz_wrong]).env("SEED", "1").env("CASES", "10000")
            .output().map_err(|e| e.to_string())?;
        require(&out, false, "minimized input hex=a5 escaped=\"\\xa5\"")?;
        require(&out, false, "divergence seed=1 case=")?;
        Ok(())
    })());
    fs::remove_dir_all(&temp).map_err(|e| format!("remove selftest store: {e}"))?;
    if failed { Err("selftest failed".into()) } else { Ok(()) }
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let result = match args.next().as_deref().and_then(|s| s.to_str()) {
        Some("run") => run_args(args.collect()),
        Some("selftest") => selftest(),
        Some("spec") => (|| {
            let id = args.next().ok_or("missing box id")?.into_string().map_err(|_| "non-UTF8 box id")?;
            if args.next().is_some() || id.is_empty() || !id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_') {
                return Err("usage: goldens spec BOX".into());
            }
            let spec = Spec::parse(&fs::read_to_string(Path::new("boxes").join(&id).join("box.toml")).map_err(|e| format!("box spec: {e}"))?)?;
            print!("{}", spec.acceptance_facts());
            Ok(())
        })(),
        Some("fuzz") => (|| {
            let id = args.next().ok_or("missing box id")?.into_string().map_err(|_| "non-UTF8 box id")?;
            let candidate = match args.next() {
                None => None,
                Some(arg) if arg == "--candidate" => Some(PathBuf::from(args.next().ok_or("missing candidate path")?)),
                _ => return Err("invalid fuzz option".into()),
            };
            if args.next().is_some() { return Err("extra fuzz arguments".into()); }
            goldens::fuzz(&id, &default_store()?, candidate)
        })(),
        _ => Err("usage: goldens selftest | spec BOX | run BOX seen|holdout [options] | fuzz BOX [--candidate PATH]".into()),
    };
    match result { Ok(()) => ExitCode::SUCCESS, Err(e) => { eprintln!("{e}"); ExitCode::FAILURE } }
}
