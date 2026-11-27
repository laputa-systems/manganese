use std::path::{Path, PathBuf};

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../webpkit-lossless-conformance/fixtures/decode")
}

fn level(meta: &str) -> Option<&str> {
    meta.lines()
        .find_map(|line| line.trim().strip_prefix("level = "))
        .map(|value| value.trim().trim_matches('"'))
}

/// Every fixture in the lossless conformance corpus decodes to the pixels libwebp
/// recorded: `expected.rgba` is `dwebp -pam` output with its header stripped, so
/// it is RGBA8 in both file order and layout. The animation's reference is its
/// first composited frame, which is what [`webpkit::decode`] returns for an
/// animation; the VP8X fixture adds metadata to the same still-image pixels.
#[test]
fn decodes_every_corpus_fixture_to_its_expected_pixels() {
    let dir = corpus_dir();
    let mut fixtures: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.is_dir())
        .collect();
    fixtures.sort();
    let mut checked = 0usize;
    for fixture in fixtures {
        let name = fixture.display();
        let meta = std::fs::read_to_string(fixture.join("meta.toml"))
            .unwrap_or_else(|e| panic!("{name}: meta.toml: {e}"));
        assert_eq!(
            level(&meta),
            Some("must"),
            "{name}: every corpus fixture is a must-pass case"
        );
        let input = std::fs::read(fixture.join("input.webp"))
            .unwrap_or_else(|e| panic!("{name}: input.webp: {e}"));
        let expected = std::fs::read(fixture.join("expected.rgba"))
            .unwrap_or_else(|e| panic!("{name}: expected.rgba: {e}"));
        let image =
            webpkit::decode(&input).unwrap_or_else(|e| panic!("{name}: decode failed: {e:?}"));
        let (width, height) = (image.dimensions().width(), image.dimensions().height());
        assert_eq!(
            expected.len(),
            width as usize * height as usize * 4,
            "{name}: expected.rgba is not RGBA8 at the decoded {width}x{height}"
        );
        assert_eq!(
            image.as_bytes(),
            expected.as_slice(),
            "{name}: decoded pixels differ from the libwebp reference"
        );
        checked += 1;
    }
    assert!(
        checked >= 16,
        "corpus shrank: only {checked} must-pass fixtures were checked"
    );
}
