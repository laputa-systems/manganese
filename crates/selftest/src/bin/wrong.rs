fn main() -> std::io::Result<()> { selftest::serve(|record| { record.make_ascii_uppercase(); record.push(b'!'); }) }
