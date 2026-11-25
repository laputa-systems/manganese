use url_parser::Url;

pub struct FileUrl {
    pub spec: String,
    pub host: Option<String>,
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
}

fn slash(c: char) -> bool { c == '/' || c == '\\' }

fn path_input(input: &str) -> String {
    let mut out = String::new();
    let mut tail = false;
    for ch in input.chars() {
        if ch == '?' || ch == '#' { tail = true; }
        match ch {
            '\\' if !tail => out.push('/'),
            '|' if !tail => out.push_str("%7C"),
            '^' if !tail => out.push_str("%5E"),
            ' ' if !tail => out.push_str("%20"),
            _ => out.push(ch),
        }
    }
    out
}

fn from_http(url: Url, has_host: bool) -> FileUrl {
    let mut host = if has_host { url.host_str().map(str::to_owned) } else { None };
    let path = url.path().to_owned();
    if host.as_deref() == Some("localhost") {
        let bytes = path.as_bytes();
        if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
            host = None;
        }
    }
    let mut spec = String::from("file://");
    if let Some(value) = &host { spec.push_str(value); }
    spec.push_str(&path);
    let query = url.query().map(str::to_owned);
    let fragment = url.fragment().map(str::to_owned);
    if let Some(value) = &query { spec.push('?'); spec.push_str(value); }
    if let Some(value) = &fragment { spec.push('#'); spec.push_str(value); }
    FileUrl { spec, host, path, query, fragment }
}

fn absolute(input: &str) -> Option<FileUrl> {
    let value = input.trim_matches(|c: char| c <= ' ');
    let tail = value.get(5..)?;
    let count = tail.chars().take_while(|c| slash(*c)).count();
    let rest = &tail[count..];
    let (authority, path, has_host) = if count == 2 {
        let end = rest.find(slash).unwrap_or(rest.len());
        let host = &rest[..end];
        if host.contains(['@', '?', '#']) || (host.contains(':') && !host.starts_with('[')) ||
           (host.starts_with('[') && !host.ends_with(']')) { return None; }
        (if host.is_empty() { "manganese.invalid" } else { host }, &rest[end..], !host.is_empty())
    } else {
        ("manganese.invalid", if count > 0 { &tail[count - 1..] } else { tail }, false)
    };
    let path = path_input(path);
    let path = if path.starts_with('/') || path.starts_with('?') || path.starts_with('#') || path.is_empty() { path } else { format!("/{path}") };
    let raw = format!("http://{authority}{path}");
    let (url, host_override) = match Url::parse(&raw) {
        Ok(url) => (url, None),
        Err(_) if has_host => {
            let (url, host) = crate::host::parse(&raw)?;
            (url, Some(host))
        }
        Err(_) => return None,
    };
    if url.username() != "" || url.password().is_some() || url.port().is_some() { return None; }
    let mut file = from_http(url, has_host);
    if let Some(host) = host_override {
        file.spec = file.spec.replacen("manganese.invalid", &host, 1);
        file.host = Some(host);
    }
    Some(file)
}

pub fn parse(input: &str, base: Option<&str>) -> Option<FileUrl> {
    let input = input.trim_matches(|c: char| c <= ' ');
    let has_scheme = input.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:"));
    if let Some(base) = base {
        let base = absolute(base)?;
        let relative = if has_scheme {
            let tail = &input[5..];
            if tail.chars().take_while(|c| slash(*c)).count() >= 2 { return absolute(input); }
            tail
        } else { input };
        if relative.chars().take_while(|c| slash(*c)).count() >= 2 {
            return absolute(&format!("file:{relative}"));
        }
        let prefix = if let Some(host) = &base.host { host.as_str() } else { "manganese.invalid" };
        let mut marker = String::from("manganese-drive-");
        while base.path.contains(&marker) || relative.contains(&marker) { marker.push('x'); }
        let marked = base.path.as_bytes().get(1).is_some_and(u8::is_ascii_alphabetic) && base.path.as_bytes().get(2) == Some(&b':');
        let path = if marked {
            format!("/{marker}{}", &base.path[1..])
        } else { base.path.clone() };
        let mut fake = format!("http://{prefix}{path}");
        if let Some(query) = &base.query { fake.push('?'); fake.push_str(query); }
        if let Some(fragment) = &base.fragment { fake.push('#'); fake.push_str(fragment); }
        let base_url = Url::parse(&fake).ok()?;
        let has_host = base.host.is_some();
        let relative = path_input(relative);
        let relative = if has_scheme && relative.find(':').is_some_and(|i| i > 0 && relative.as_bytes()[..i].iter().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(b))) {
            format!("./{relative}")
        } else { relative };
        let url = base_url.join(&relative).ok()?;
        let mut file = from_http(url, has_host);
        let prefix = format!("/{marker}");
        if let Some(rest) = file.path.strip_prefix(&prefix).filter(|_| marked) {
            file.path = format!("/{rest}");
            file.spec = file.spec.replacen(&prefix, "/", 1);
        }
        if file.host.is_none() && file.path.starts_with("//") {
            file.spec.insert_str("file://".len(), "/.");
        }
        Some(file)
    } else if has_scheme { absolute(input) } else { None }
}

pub fn replace_path(file: &FileUrl, value: &str) -> Option<FileUrl> {
    let host = file.host.as_deref().unwrap_or("manganese.invalid");
    let value: String = value.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let path = path_input(&value.replace('?', "%3F").replace('#', "%23"));
    let path = if path.starts_with('/') { path } else { format!("/{path}") };
    let mut url = Url::parse(&format!("http://{host}{path}")).ok()?;
    url.set_query(file.query.as_deref());
    url.set_fragment(file.fragment.as_deref());
    Some(from_http(url, file.host.is_some()))
}

fn rebuild(file: &mut FileUrl) {
    file.spec = format!("file://{}{}", file.host.as_deref().unwrap_or(""), file.path);
    if let Some(query) = &file.query { file.spec.push('?'); file.spec.push_str(query); }
    if let Some(fragment) = &file.fragment { file.spec.push('#'); file.spec.push_str(fragment); }
}

pub fn replace_host(file: &mut FileUrl, value: &str) -> bool {
    let host = if value.is_empty() { None } else {
        let raw = format!("http://{value}/");
        match Url::parse(&raw) {
            Ok(url) if url.username().is_empty() && url.password().is_none() && url.port().is_none() => url.host_str().map(str::to_owned),
            _ => match crate::host::parse(&raw) { Some((_, host)) => Some(host), None => return false },
        }
    };
    file.host = host;
    if file.host.as_deref() == Some("localhost") {
        let path = file.path.as_bytes();
        if path.len() >= 3 && path[0] == b'/' && path[1].is_ascii_alphabetic() && path[2] == b':' { file.host = None; }
    }
    rebuild(file);
    true
}

pub fn replace_query_or_fragment(file: &mut FileUrl, name: &str, value: &str) {
    let clean: String = value.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let mut helper = Url::parse("http://manganese.invalid/").unwrap();
    if name == "search" {
        if value.is_empty() || value == "?" { file.query = None; }
        else {
            helper.set_query(Some(clean.strip_prefix('?').unwrap_or(&clean)));
            file.query = helper.query().map(str::to_owned);
        }
    } else if value.is_empty() { file.fragment = None; }
    else {
        helper.set_fragment(Some(clean.strip_prefix('#').unwrap_or(&clean)));
        file.fragment = helper.fragment().map(str::to_owned);
    }
    rebuild(file);
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn wpt_119_file_relative_drive_pipe() {
        assert_eq!(parse("C|/foo/bar", Some("file:///tmp/mock/path")).unwrap().spec, "file:///tmp/mock/C%7C/foo/bar");
    }

    #[test]
    fn wpt_131_file_localhost_is_preserved() {
        assert_eq!(parse("file://localhost", None).unwrap().spec, "file://localhost/");
    }

    #[test]
    fn wpt_613_file_parent_can_remove_drive() {
        assert_eq!(parse("..", Some("file:///C:/")).unwrap().spec, "file:///");
    }

    #[test]
    fn wpt_619_file_drive_authority_is_invalid() {
        assert!(parse("//d:", Some("file:///C:/a/b")).is_none());
    }

    #[test]
    fn wpt_117_file_same_scheme_relative_drive() {
        assert_eq!(parse("file:c:\\foo\\bar.html", Some("file:///tmp/mock/path")).unwrap().spec, "file:///tmp/mock/c:/foo/bar.html");
    }

    #[test]
    fn wpt_672_file_same_scheme_drive_keeps_host() {
        assert_eq!(parse("file:C:/", Some("file://host/")).unwrap().spec, "file://host/C:/");
    }

    #[test]
    fn wpt_843_file_ascii_host_is_not_revalidated_as_idna() {
        assert_eq!(parse("file://xn--/p", None).unwrap().spec, "file://xn--/p");
    }

    #[test]
    fn file_relative_double_slash_path_is_disambiguated() {
        let value = parse("..//", Some("file:///tmp/a")).unwrap();
        assert_eq!(value.path, "//");
        assert_eq!(value.spec, "file:///.//");
    }

    #[test]
    fn file_host_without_path_cannot_contain_query() {
        assert!(parse("file://example.com?x", None).is_none());
    }

    #[test]
    fn file_ipv6_host_cannot_include_a_port() {
        assert!(parse("file://[::1]:80", None).is_none());
    }
}
