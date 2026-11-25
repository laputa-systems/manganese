use url_parser::{Origin, Position, Url};

pub struct FilesystemUrl {
    pub spec: String,
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
    pub origin: String,
    pub opaque: bool,
}

pub fn parse(input: &str) -> Option<FilesystemUrl> {
    let input = input.trim_matches(|c: char| c <= ' ');
    if !input.get(..11)?.eq_ignore_ascii_case("filesystem:") { return None; }
    let inner = &input[11..];
    let mut url = Url::parse(inner).ok()?;
    if !matches!(url.scheme(), "http" | "https" | "ftp" | "ws" | "wss" | "file") { return None; }
    let path = url.path();
    let end = path[1..].find('/').map_or(path.len(), |i| i + 1);
    if end <= 1 { return None; }
    let kind = path[..end].to_owned();
    let outer = if end == path.len() { "/".to_owned() } else { path[end..].to_owned() };
    let query = url.query().map(str::to_owned);
    let fragment = url.fragment().map(str::to_owned);
    let origin = if url.scheme() == "file" { "file://".to_owned() } else { url.origin().ascii_serialization() };
    let opaque = url.scheme() != "file" && matches!(url.origin(), Origin::Opaque(_));
    if !url.username().is_empty() || url.password().is_some() {
        url.set_username("").ok()?;
        url.set_password(None).ok()?;
    }
    let mut spec = format!("filesystem:{}{kind}{outer}", &url[..Position::BeforePath]);
    if let Some(value) = &query { spec.push('?'); spec.push_str(value); }
    if let Some(value) = &fragment { spec.push('#'); spec.push_str(value); }
    Some(FilesystemUrl { spec, path: outer, query, fragment, origin, opaque })
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn filesystem_requires_a_valid_inner_origin_and_type() {
        assert!(parse("filesystem://[::1]/a%2fb").is_none());
        assert!(parse("filesystem:https://example.com/").is_none());
        let url = parse("filesystem:https://example.com/temporary/a").unwrap();
        assert_eq!(url.spec, "filesystem:https://example.com/temporary/a");
        assert_eq!(url.path, "/a");
        assert_eq!(url.origin, "https://example.com");
    }
}
