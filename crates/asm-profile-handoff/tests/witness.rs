use asm_profile_handoff::{
    ALPHA, BETA, CHANGED_ALPHA, GAMMA, INDEXED, BrowserOutcome, browser, entries, hydrogen_profile,
    indexeddb_db, raw_get, repo_root, storage_key, truncate_active_log, write, write_raw,
};
const INCOMPATIBLE: &str = "manganese-incompatible";

fn changed(profile: &std::path::Path) {
    write(profile, &[("alpha", Some(CHANGED_ALPHA)), ("beta", None), ("gamma", Some(GAMMA))]);
}

fn assert_indexeddb_present(profile: &std::path::Path) {
    assert!(indexeddb_db(profile).join("CURRENT").is_file(), "the IndexedDB database is still in the profile");
}

fn assert_browser_saw(outcome: &BrowserOutcome, expected: &[&str]) {
    assert!(outcome.ok, "the frozen browser reopened the profile: {}", outcome.detail());
    for text in expected {
        assert!(outcome.stdout.contains(text), "missing {text:?} in {}", outcome.stdout.trim());
    }
}

#[test]
fn step1_frozen_browser_writes_a_profile_the_rust_reader_opens() {
    let root = repo_root();
    let profile = hydrogen_profile(&root);
    let values = entries(&profile);
    assert_eq!(values.get("alpha").map(String::as_str), Some(ALPHA));
    assert_eq!(values.get("beta").map(String::as_str), Some(BETA));
    assert!(!values.contains_key("removed"), "the browser's delete reached the database");
    assert_eq!(raw_get(&profile, b"VERSION"), Some(b"1".to_vec()));
    assert_indexeddb_present(&profile);
    println!("step 1: Rust read {} Local Storage entries from the Hydrogen profile", values.len());
}

#[test]
fn step2_rust_changes_state_flushes_and_reopens_it() {
    let profile = hydrogen_profile(&repo_root());
    changed(&profile);
    let first = entries(&profile);
    assert_eq!(first.get("alpha").map(String::as_str), Some(CHANGED_ALPHA));
    assert_eq!(first.get("gamma").map(String::as_str), Some(GAMMA));
    assert!(!first.contains_key("beta"), "the selected state was deleted");
    let reopened = entries(&profile);
    assert_eq!(first, reopened, "the flush survives a close and reopen in the Rust reader");
    assert_indexeddb_present(&profile);
    println!("step 2: Rust wrote, flushed and reopened {} entries", reopened.len());
}

#[test]
fn step3_frozen_browser_reopens_the_manganese_written_profile() {
    let root = repo_root();
    let profile = hydrogen_profile(&root);
    changed(&profile);
    let outcome = browser(&root, "read", &profile, Some(("gamma", GAMMA)));
    assert_browser_saw(&outcome, &[INDEXED, "\"beta\": null", "\"local\": 2"]);
    println!("step 3: the frozen browser observed {}", outcome.stdout.trim());
}

#[test]
fn step4_interrupted_write_recovers_for_both_readers() {
    let root = repo_root();
    let profile = hydrogen_profile(&root);
    write(&profile, &[("alpha", Some("manganese-interrupted"))]);
    let log = truncate_active_log(&profile);
    let recovered = entries(&profile);
    assert_eq!(recovered.get("alpha").map(String::as_str), Some(ALPHA), "the interrupted write did not replace committed state");
    assert_eq!(raw_get(&profile, b"VERSION"), Some(b"1".to_vec()));
    assert_indexeddb_present(&profile);
    let outcome = browser(&root, "read", &profile, Some(("alpha", ALPHA)));
    assert_browser_saw(&outcome, &[INDEXED]);
    println!("step 4: recovery after truncating {} kept both readers on the committed profile", log.display());
}

#[test]
fn negative_control_incompatible_value_breaks_the_browser_reopen() {
    let root = repo_root();
    let profile = hydrogen_profile(&root);
    write_raw(&profile, "alpha", INCOMPATIBLE.as_bytes());
    assert_eq!(raw_get(&profile, &storage_key("alpha")), Some(INCOMPATIBLE.as_bytes().to_vec()));
    let outcome = browser(&root, "read", &profile, Some(("alpha", INCOMPATIBLE)));
    assert!(!outcome.ok, "an incompatible value must not be observed by the frozen browser: {}", outcome.detail());
    println!("negative control: the browser rejected the incompatible value ({})", outcome.detail());
}
