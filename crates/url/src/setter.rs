use url_parser::{Position, Url};

fn clean(value: &str) -> String {
    value.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect()
}

fn escape_controls(value: &str) -> String {
    let mut result = String::new();
    for ch in value.chars() {
        if ch <= ' ' { result.push_str(&format!("%{:02X}", ch as u32)); }
        else { result.push(ch); }
    }
    result
}

fn port(value: &str) -> Option<u16> {
    let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() { None } else { digits.parse().ok() }
}

pub fn apply(input: &str, base: Option<&str>, name: &str, value: &str) -> Option<String> {
    let direct = name.starts_with("blink:");
    let name = name.strip_prefix("blink:").unwrap_or(name);
    if let Some(result) = apply_inner(input, base, name, value, direct) { return Some(result); }
    if base.is_some_and(|value| Url::parse(value).is_err()) { return None; }
    let (url, host) = crate::host::parse(input)?;
    let result = apply_inner(url.as_str(), None, name, value, direct)?;
    if name != "href" {
        if let Ok(parsed) = Url::parse(&result) {
            if parsed.host_str() == Some("manganese.invalid") {
                return Some(format!("{}{host}{}", &parsed[..Position::BeforeHost], &parsed[Position::AfterHost..]));
            }
        }
    }
    Some(result)
}

fn apply_inner(input: &str, base: Option<&str>, name: &str, value: &str, direct: bool) -> Option<String> {
    let mut url = match base { Some(base) => Url::parse(base).ok()?.join(input).ok()?, None => Url::parse(input).ok()? };
    let has_scheme = input.find(':').is_some_and(|index| index > 0 && input.as_bytes()[0].is_ascii_alphabetic() && input.as_bytes()[..index].iter().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(b)));
    let file_base = base.is_some_and(|s| s.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("file:")));
    let original_file = if input.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:")) || (file_base && !has_scheme) {
        crate::file_url::parse(input, if file_base { base } else { None })
    } else { None };
    let file = original_file.as_ref().and_then(|value| crate::file_url::parse(&value.spec, None));
    if let Some(file) = &file {
        match name {
            "username" | "password" | "port" => return Some(file.spec.clone()),
            "host" | "hostname" => {
                let cleaned = clean(value);
                let end = cleaned.find(['/', '\\', '?', '#']).unwrap_or(cleaned.len());
                let value = &cleaned[..end];
                let bracket = value.rfind(']').map_or(0, |index| index + 1);
                let colon = value[bracket..].find(':').map(|index| index + bracket);
                if name == "hostname" && colon.is_some() { return original_file.map(|value| value.spec); }
                let value = if name == "host" { colon.map_or(value, |index| &value[..index]) } else { value };
                let tail = &file.spec["file://".len() + file.host.as_deref().map_or(0, str::len)..];
                let changed = format!("file://{}{}", clean(value), tail);
                return Some(crate::file_url::parse(&changed, None).map_or_else(|| original_file.as_ref().unwrap().spec.clone(), |url| url.spec));
            }
            "pathname" => {
                let path = escape_controls(&clean(value)).replace('?', "%3F").replace('#', "%23");
                let host = file.host.as_deref().unwrap_or("");
                let path = if path.starts_with(['/', '\\']) { path } else { format!("/{path}") };
                let mut changed = format!("file://{host}{path}");
                if let Some(query) = &file.query { changed.push('?'); changed.push_str(query); }
                if let Some(fragment) = &file.fragment { changed.push('#'); changed.push_str(fragment); }
                return Some(crate::file_url::parse(&changed, None).map_or_else(|| file.spec.clone(), |url| url.spec));
            }
            "search" => {
                let end = file.spec.find(['?', '#']).unwrap_or(file.spec.len());
                let mut changed = file.spec[..end].to_owned();
                if !value.is_empty() && value != "?" {
                    let query = clean(value);
                    changed.push('?');
                    changed.push_str(&escape_controls(query.strip_prefix('?').unwrap_or(&query)).replace('#', "%23"));
                }
                if let Some(fragment) = &file.fragment { changed.push('#'); changed.push_str(fragment); }
                return Some(changed);
            }
            "hash" => {
                let end = file.spec.find('#').unwrap_or(file.spec.len());
                let mut changed = file.spec[..end].to_owned();
                if !value.is_empty() {
                    let fragment = clean(value);
                    changed.push('#');
                    changed.push_str(&escape_controls(fragment.strip_prefix('#').unwrap_or(&fragment)));
                }
                return Some(changed);
            }
            _ => {}
        }
    }
    match name {
        "href" => return match Url::parse(value) {
            Ok(value) => Some(value.to_string()),
            Err(_) if direct => Some(original_file.as_ref().map_or_else(|| url.to_string(), |file| file.spec.clone())),
            Err(_) => None,
        },
        "protocol" => {
            let scheme = value.split(':').next()?;
            if scheme.is_empty() || !scheme.as_bytes()[0].is_ascii_alphabetic() || !scheme.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b)) {
                return Some(file.as_ref().map_or_else(|| url.to_string(), |file| file.spec.clone()));
            }
            let original = file.as_ref().map_or_else(|| url.to_string(), |file| file.spec.clone());
            let old_special = matches!(url.scheme(), "http" | "https" | "ftp" | "ws" | "wss" | "file");
            let new_special = matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "ftp" | "ws" | "wss" | "file");
            if old_special && !new_special { return Some(original); }
            if old_special && scheme.eq_ignore_ascii_case("file") && (!url.username().is_empty() || url.password().is_some() || url.port().is_some()) { return Some(original); }
            if file.as_ref().is_some_and(|file| file.host.is_none()) && !scheme.eq_ignore_ascii_case("file") { return Some(original); }
            let colon = original.find(':')?;
            let changed = format!("{scheme}{}", &original[colon..]);
            let valid = if scheme.eq_ignore_ascii_case("file") { crate::file_url::parse(&changed, None).is_some() }
                        else { Url::parse(&changed).is_ok() || crate::host::parse(&changed).is_some() };
            return if valid { Some(changed) } else if direct { None } else { Some(original) };
        }
        "username" => { let _ = url.set_username(value); }
        "password" => { let _ = url.set_password(if value.is_empty() { None } else { Some(value) }); }
        "hostname" | "host" if !url.cannot_be_a_base() => {
            let cleaned = clean(value);
            let special = matches!(url.scheme(), "http" | "https" | "ftp" | "ws" | "wss" | "file");
            let end = cleaned.find(|c| c == '/' || c == '?' || c == '#' || (special && c == '\\')).unwrap_or(cleaned.len());
            let host_and_port = &cleaned[..end];
            if host_and_port.contains('@') || (!special && host_and_port.contains([' ', '\\'])) { return Some(url.to_string()); }
            let bracket = host_and_port.rfind(']').map_or(0, |i| i + 1);
            let colon = host_and_port[bracket..].find(':').map(|i| i + bracket);
            if name == "host" {
                if colon != Some(0) {
                    let host = colon.map_or(host_and_port, |i| &host_and_port[..i]);
                    if host.is_empty() && (!url.username().is_empty() || url.password().is_some() || url.port().is_some()) {
                        if value.is_empty() || special { return Some(url.to_string()); }
                        return Some(format!("{}://{}", url.scheme(), &url[Position::BeforePath..]));
                    }
                    let original = url.clone();
                    if (!host.is_empty() || !special) && url.set_host(Some(host)).is_ok() {
                        if let Some(number) = colon.and_then(|i| port(&host_and_port[i + 1..])) { let _ = url.set_port(Some(number)); }
                    } else if let Some((_, host)) = crate::host::parse(&format!("http://{host}/")) {
                        return Some(format!("{}{host}{}", &original[..Position::BeforeHost], &original[Position::AfterHost..]));
                    } else {
                        url = original;
                    }
                }
            } else if !host_and_port.is_empty() || !special {
                if colon.is_some() { return Some(url.to_string()); }
                if host_and_port.is_empty() && (!url.username().is_empty() || url.password().is_some() || url.port().is_some()) {
                    if value.is_empty() || special { return Some(url.to_string()); }
                    return Some(format!("{}://{}", url.scheme(), &url[Position::BeforePath..]));
                }
                let original = url.clone();
                if url.set_host(Some(host_and_port)).is_err() {
                    if let Some((_, host)) = crate::host::parse(&format!("http://{host_and_port}/")) {
                        return Some(format!("{}{host}{}", &original[..Position::BeforeHost], &original[Position::AfterHost..]));
                    }
                    url = original;
                }
            }
        }
        "port" if !url.cannot_be_a_base() => {
            if value.is_empty() { let _ = url.set_port(None); }
            else if let Some(number) = port(&clean(value)) { let _ = url.set_port(Some(number)); }
        }
        "pathname" if !url.cannot_be_a_base() => {
            let path = clean(value).replace('?', "%3F").replace('#', "%23");
            let empty_path = if url.has_authority() && !matches!(url.scheme(), "http" | "https" | "ftp" | "ws" | "wss" | "file") { "" } else { "/" };
            url.set_path(if path.is_empty() { empty_path } else { &path });
            if url.host_str().is_none() && url.path().starts_with("//") {
                let mut result = format!("{}:/.{}", url.scheme(), url.path());
                if let Some(query) = url.query() { result.push('?'); result.push_str(query); }
                if let Some(fragment) = url.fragment() { result.push('#'); result.push_str(fragment); }
                return Some(result);
            }
        }
        "search" => {
            if value.is_empty() || value == "?" { url.set_query(None); }
            else { let value = clean(value); url.set_query(Some(value.strip_prefix('?').unwrap_or(&value))); }
        }
        "hash" => {
            if value.is_empty() { url.set_fragment(None); }
            else { let value = clean(value); url.set_fragment(Some(value.strip_prefix('#').unwrap_or(&value))); }
        }
        "hostname" | "host" | "port" | "pathname" => {}
        _ => return None,
    }
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::apply;

    #[test]
    fn wpt_setters_protocol_port_search_and_hash() {
        for (input, name, value, expected) in [
            ("a://example.net", "protocol", "b", "b://example.net"),
            ("http://example.net", "port", "8080", "http://example.net:8080/"),
            ("https://example.net#nav", "search", "lang=fr", "https://example.net/?lang=fr#nav"),
            ("https://example.net", "hash", "main", "https://example.net/#main"),
        ] {
            assert_eq!(apply(input, None, name, value).as_deref(), Some(expected));
        }
    }

    #[test]
    fn wpt_setters_preserve_file_and_invalid_host_semantics() {
        for (input, name, value, expected) in [
            ("file://localhost/", "protocol", "http", "http://localhost/"),
            ("file://hi/x", "host", "", "file:///x"),
            ("file://y/", "host", "x:123", "file://x/"),
            ("http://example.net/path", "hostname", "example.com:8080", "http://example.net/path"),
            ("https://test.invalid/", "host", "x@x", "https://test.invalid/"),
            ("file://monkey/", "pathname", "\\\\", "file://monkey//"),
            ("file://example.com/a", "pathname", " ", "file://example.com/%20"),
        ] {
            assert_eq!(apply(input, None, name, value).as_deref(), Some(expected));
        }
    }
}
