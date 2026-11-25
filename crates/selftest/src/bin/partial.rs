fn main() -> std::io::Result<()> {
    selftest::serve(|record| {
        record.make_ascii_uppercase();
        if record == b"BETA" { record.push(b'!'); }
    })
}
