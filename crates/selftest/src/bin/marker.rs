fn main() -> std::io::Result<()> {
    let path = std::env::var_os("MANGANESE_MARKER").ok_or_else(|| std::io::Error::other("missing marker path"))?;
    std::fs::write(path, b"candidate ran")
}
