fn main() -> std::io::Result<()> {
    selftest::serve(|record| {
        let injected = record.contains(&0xa5);
        record.make_ascii_uppercase();
        if injected { record.push(b'!'); }
    })
}
