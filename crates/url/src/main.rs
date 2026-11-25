use publicsuffix::{IcannList, List, Psl};
use std::io::{self, Read, Write};
use std::sync::OnceLock;
use url_parser::{Host, Origin, Position, Url};

mod file_url;
mod filesystem_url;
mod host;
mod setter;

const SUFFIX_DATA: &[u8] = include_bytes!("effective_tld_names.dat");

struct SuffixLists {
    all: List,
    icann: IcannList,
}

fn suffix_lists() -> &'static SuffixLists {
    static LISTS: OnceLock<SuffixLists> = OnceLock::new();
    LISTS.get_or_init(|| {
        let all = List::from_bytes(SUFFIX_DATA).expect("Chromium suffix list must parse");
        SuffixLists { icann: all.clone().into(), all }
    })
}

fn registrable_domain(host: &str, include_private: bool) -> Option<&str> {
    let ip = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if host.is_empty() || ip.parse::<std::net::IpAddr>().is_ok() || matches!(Host::parse(host), Ok(Host::Ipv4(_) | Host::Ipv6(_))) {
        return None;
    }
    if host.ends_with("..") { return None; }
    let host = host.trim_start_matches('.');
    if host.is_empty() {
        return None;
    }
    let lists = suffix_lists();
    let domain = if include_private {
        lists.all.domain(host.as_bytes())
    } else {
        lists.icann.domain(host.as_bytes())
    }?;
    std::str::from_utf8(domain.as_bytes()).ok()
}

fn take_field<'a>(data: &mut &'a [u8]) -> io::Result<Option<&'a str>> {
    if data.len() < 4 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "missing field length"));
    }
    let (length, tail) = data.split_at(4);
    *data = tail;
    let length = u32::from_be_bytes(length.try_into().unwrap());
    if length == u32::MAX {
        return Ok(None);
    }
    let length = length as usize;
    if data.len() < length {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short field"));
    }
    let (field, tail) = data.split_at(length);
    *data = tail;
    Ok(Some(std::str::from_utf8(field).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?))
}

fn put_field(output: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(value) => {
            output.extend_from_slice(&(value.len() as u32).to_be_bytes());
            output.extend_from_slice(value.as_bytes());
        }
        None => output.extend_from_slice(&u32::MAX.to_be_bytes()),
    }
}

fn site(response: &[u8]) -> io::Result<Option<(String, String)>> {
    if response.first() != Some(&1) || response.get(1) != Some(&0) { return Ok(None); }
    let mut fields = &response[2..];
    let mut values = Vec::new();
    for _ in 0..11 { values.push(take_field(&mut fields)?); }
    let origin = values[9].unwrap_or("null");
    let scheme = origin.split(':').next().unwrap_or("");
    let host = if scheme == "file" {
        if let Some(value) = values[4] { value.to_owned() }
        else if let Some(inner) = values[0].and_then(|value| value.strip_prefix("blob:")) {
            file_url::parse(inner, None).and_then(|value| value.host).unwrap_or_default()
        } else { String::new() }
    } else if let Ok(value) = Url::parse(origin) {
        value.host_str().unwrap_or("").to_owned()
    } else if let Some((_, host)) = host::parse(origin) { host }
    else { return Ok(None); };
    let domain = registrable_domain(&host, true).unwrap_or(&host).to_owned();
    Ok(Some((scheme.to_owned(), domain)))
}

fn file_response(file: file_url::FileUrl, other: Option<&str>, include_private: bool) -> Vec<u8> {
    let domain = file.host.as_deref().and_then(|host| registrable_domain(host, include_private));
    let same = other.and_then(|value| Url::parse(value).ok()).and_then(|value| {
        let other_host = value.host_str()?;
        let host = file.host.as_deref()?;
        Some(domain.unwrap_or(host) == registrable_domain(other_host, include_private).unwrap_or(other_host))
    });
    let mut output = vec![1, 0];
    for field in [Some(file.spec.as_str()), Some("file"), Some(""), None, file.host.as_deref(), None,
                  Some(file.path.as_str()), file.query.as_deref(), file.fragment.as_deref(), Some("file://"), domain] {
        put_field(&mut output, field);
    }
    output.push(match same { Some(true) => 1, Some(false) => 0, None => 2 });
    output
}

fn parse(input: &str, base: Option<&str>) -> Result<Url, url_parser::ParseError> {
    match base {
        Some(base) => {
            let mut base = Url::parse(base)?;
            let count = input.chars().take_while(|c| *c == '/' || *c == '\\').count();
            let special = matches!(base.scheme(), "http" | "https" | "ftp" | "ws" | "wss");
            let mut marker = String::from("manganese-drive-");
            while base.as_str().contains(&marker) || input.contains(&marker) { marker.push('x'); }
            let mut marked = false;
            let path = base.path().split('/').map(|part| {
                let bytes = part.as_bytes();
                if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
                    marked = true;
                    format!("{marker}{part}")
                } else { part.to_owned() }
            }).collect::<Vec<_>>().join("/");
            if marked { base.set_path(&path); }
            let mut result = if !special && input.starts_with('\\') && count == 1 {
                base.join(&format!("/{}", &input[1..]))
            } else if !special && input.starts_with('\\') {
                Url::parse(&format!("{}:/{}", base.scheme(), input))
            } else if count > 2 && special {
                base.join(&format!("//{}", &input[count..]))
            } else {
                base.join(input)
            }?;
            if marked { result.set_path(&result.path().replace(&marker, "")); }
            Ok(result)
        }
        None if input.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("data:")) && input.contains('#') => {
            let (head, fragment) = input.split_once('#').unwrap();
            let fragment = fragment.replace('\t', "%09").replace('\n', "%0A").replace('\r', "%0D");
            Url::parse(&format!("{head}#{fragment}"))
        }
        None => Url::parse(input),
    }
}

fn respond(mut request: &[u8]) -> io::Result<Vec<u8>> {
    let input = take_field(&mut request)?.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing URL"))?;
    let base = take_field(&mut request)?;
    let other = take_field(&mut request)?;
    if request.is_empty() || request[0] > 1 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid registry policy"));
    }
    let include_private = request[0] == 1;
    request = &request[1..];
    if !request.is_empty() {
        let name = take_field(&mut request)?.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing setter"))?;
        let value = take_field(&mut request)?.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing setter value"))?;
        if !request.is_empty() { return Err(io::Error::new(io::ErrorKind::InvalidData, "extra setter fields")); }
        let mut original_request = Vec::new();
        put_field(&mut original_request, Some(input));
        put_field(&mut original_request, base);
        put_field(&mut original_request, other);
        original_request.push(u8::from(include_private));
        let original = respond(&original_request)?;
        if original.first() != Some(&1) { return Ok(original); }
        if name == "schemeful-site" {
            let mut result = original;
            let same = if let Some(other) = other {
                let mut other_request = Vec::new();
                put_field(&mut other_request, Some(other));
                put_field(&mut other_request, None);
                put_field(&mut other_request, None);
                other_request.push(u8::from(include_private));
                let first = site(&result)?;
                let second = site(&respond(&other_request)?)?;
                u8::from(first.is_some() && first == second)
            } else { 2 };
            *result.last_mut().unwrap() = same;
            return Ok(result);
        }
        if name == "blink:href" {
            let mut replacement = Vec::new();
            put_field(&mut replacement, Some(value));
            put_field(&mut replacement, None);
            put_field(&mut replacement, other);
            replacement.push(u8::from(include_private));
            let updated = respond(&replacement)?;
            return if updated.first() == Some(&1) { Ok(updated) } else { Ok(original) };
        }
        if name.starts_with("blink:") {
            let mut fields = &original[2..];
            let original_spec = take_field(&mut fields)?.unwrap_or("");
            if original_spec.starts_with("file:") {
                if let Some(mut file) = file_url::parse(original_spec, None) {
                    match name {
                        "blink:pathname" => {
                            if let Some(file) = file_url::replace_path(&file, value) { return Ok(file_response(file, other, include_private)); }
                            return Ok(original);
                        }
                        "blink:host" | "blink:hostname" => {
                            let clean: String = value.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
                            let end = clean.find(['/', '\\', '?', '#']).unwrap_or(clean.len());
                            let host = &clean[..end];
                            let bracket = host.rfind(']').map_or(0, |index| index + 1);
                            let colon = host[bracket..].find(':').map(|index| index + bracket);
                            if host.contains('@') || (name == "blink:hostname" && colon.is_some()) { return Ok(original); }
                            let host = if name == "blink:host" { colon.map_or(host, |index| &host[..index]) } else { host };
                            if file_url::replace_host(&mut file, host) { return Ok(file_response(file, other, include_private)); }
                            return Ok(original);
                        }
                        "blink:username" | "blink:password" | "blink:port" => return Ok(file_response(file, other, include_private)),
                        "blink:protocol" => {
                            let scheme = value.split(':').next().unwrap_or("");
                            let valid = !scheme.is_empty() && scheme.as_bytes()[0].is_ascii_alphabetic() && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b));
                            let scheme = scheme.to_ascii_lowercase();
                            if !valid || !matches!(scheme.as_str(), "http" | "https" | "ftp" | "ws" | "wss" | "file") || (scheme != "file" && file.host.is_none()) {
                                return Ok(file_response(file, other, include_private));
                            }
                        }
                        "blink:search" | "blink:hash" => {
                            file_url::replace_query_or_fragment(&mut file, &name[6..], value);
                            return Ok(file_response(file, other, include_private));
                        }
                        _ => {}
                    }
                }
            }
        }
        if (name == "blink:search" || name == "blink:hash") && value.is_empty() && original[1] == 1 {
            let mut fields = &original[2..];
            let mut values = Vec::new();
            for _ in 0..11 { values.push(take_field(&mut fields)?); }
            if values[6].is_some_and(|path| path.ends_with(' ')) {
                let old_spec = values[0].unwrap_or("");
                let spec = if name == "blink:search" {
                    let end = old_spec.find(['?', '#']).unwrap_or(old_spec.len());
                    let mut spec = old_spec[..end].to_owned();
                    if let Some(fragment) = values[8] { spec.push('#'); spec.push_str(fragment); }
                    values[7] = None;
                    spec
                } else {
                    values[8] = None;
                    old_spec[..old_spec.find('#').unwrap_or(old_spec.len())].to_owned()
                };
                let mut output = vec![1, original[1]];
                put_field(&mut output, Some(&spec));
                for value in &values[1..] { put_field(&mut output, *value); }
                output.push(*original.last().unwrap());
                return Ok(output);
            }
        }
        let updated = match setter::apply(input, base, name, value) {
            Some(value) => value,
            None => return Ok(vec![0]),
        };
        if original.first() == Some(&1) && take_field(&mut &original[2..])? == Some(updated.as_str()) {
            return Ok(original);
        }
        let mut replacement = Vec::new();
        put_field(&mut replacement, Some(&updated));
        put_field(&mut replacement, None);
        put_field(&mut replacement, other);
        replacement.push(u8::from(include_private));
        return respond(&replacement);
    }
    let trimmed = input.trim_matches(|c: char| c <= ' ');
    let file_scheme = trimmed.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:"));
    let input_scheme = trimmed.find(':').is_some_and(|i| {
        i > 0 && trimmed.as_bytes()[0].is_ascii_alphabetic() &&
        trimmed.as_bytes()[..i].iter().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(b))
    });
    let file_base = base.is_some_and(|s| s.trim_matches(|c: char| c <= ' ').get(..5).is_some_and(|p| p.eq_ignore_ascii_case("file:")));
    if trimmed.get(..11).is_some_and(|s| s.eq_ignore_ascii_case("filesystem:")) {
        let value = match filesystem_url::parse(trimmed) {
            Some(value) => value,
            None => return Ok(vec![0]),
        };
        let mut output = vec![1, u8::from(value.opaque)];
        for field in [Some(value.spec.as_str()), Some("filesystem"), Some(""), None, None, None,
                      Some(value.path.as_str()), value.query.as_deref(), value.fragment.as_deref(), Some(value.origin.as_str()), None] {
            put_field(&mut output, field);
        }
        output.push(2);
        return Ok(output);
    }
    if file_scheme || (file_base && !input_scheme) {
        let file = match file_url::parse(input, if file_base { base } else { None }) {
            Some(file) => file,
            None => return Ok(vec![0]),
        };
        return Ok(file_response(file, other, include_private));
    }
    let (parsed, host_override) = match parse(input, base) {
        Ok(parsed) => (parsed, None),
        Err(_) if base.is_none_or(|value| Url::parse(value).is_ok()) => match host::parse(input) {
            Some((parsed, host)) => (parsed, Some(host)),
            None => return Ok(vec![0]),
        },
        Err(_) => return Ok(vec![0]),
    };
    if matches!(parsed.scheme(), "android" | "drivefs" | "chromeos-steam" | "steam" | "materialized-view") {
        let path = &parsed[Position::AfterScheme..Position::AfterPath][1..];
        let mut output = vec![1, 1];
        for field in [Some(parsed.as_str()), Some(parsed.scheme()), Some(""), None, None, None,
                      Some(path), parsed.query(), parsed.fragment(), Some("null"), None] {
            put_field(&mut output, field);
        }
        output.push(2);
        return Ok(output);
    }
    let origin = parsed.origin();
    let blob_inner = parsed.as_str().strip_prefix("blob:");
    let blob_file = blob_inner.is_some_and(|inner| inner.starts_with("file:"));
    let blob_nested = blob_inner.is_some_and(|inner| inner.starts_with("blob:"));
    let serialized_origin = if blob_file { "file://".to_owned() } else if blob_nested { "null".to_owned() } else {
        let value = origin.ascii_serialization().replace('*', "%2A");
        if let Some(host) = &host_override { value.replace("manganese.invalid", host) } else { value }
    };
    let port = parsed.port().map(|value| value.to_string());
    let special = matches!(parsed.scheme(), "http" | "https" | "ftp" | "ws" | "wss");
    let host_owned = host_override.or_else(|| parsed.host_str().map(|value| if special { value.replace('*', "%2A") } else { value.to_owned() }));
    let host = host_owned.as_deref();
    let domain = host.and_then(|value| registrable_domain(value, include_private));
    let same = other.and_then(|value| Url::parse(value).ok()).and_then(|value| {
        let other_host = value.host_str()?;
        let this_host = host?;
        let a = domain.unwrap_or(this_host);
        let b = registrable_domain(other_host, include_private).unwrap_or(other_host);
        Some(a == b)
    });
    let username = parsed.username().replace('\'', "%27");
    let password = parsed.password().map(|value| value.replace('\'', "%27"));
    let path = if parsed.cannot_be_a_base() { parsed.path().to_owned() } else { parsed.path().replace('^', "%5E").replace('|', "%7C") };
    let query = parsed.query().map(|value| value.replace('\'', "%27"));
    let mut spec = String::new();
    if parsed.has_authority() {
        spec.push_str(&parsed[..Position::BeforeUsername]);
        spec.push_str(&username);
        spec.push_str(&parsed[Position::AfterUsername..Position::BeforeHost].replace('\'', "%27"));
        if let Some(value) = host { spec.push_str(value); }
        spec.push_str(&parsed[Position::AfterHost..Position::BeforePath]);
    } else {
        spec.push_str(&parsed[..Position::BeforePath]);
    }
    spec.push_str(&path);
    if let Some(value) = &query { spec.push('?'); spec.push_str(value); }
    if let Some(value) = parsed.fragment() { spec.push('#'); spec.push_str(value); }
    if !input_scheme && !special {
        if let Some(base_url) = base.and_then(|value| Url::parse(value).ok()) {
            if base_url.scheme() == parsed.scheme() && base_url.host_str().is_some() && base_url.path().is_empty() {
                spec = base_url.as_str().to_owned();
                spec.push_str(&path);
                if let Some(value) = &query { spec.push('?'); spec.push_str(value); }
                if let Some(value) = parsed.fragment() { spec.push('#'); spec.push_str(value); }
            } else if input.starts_with(['?', '#']) && base_url.host_str().is_none() && base_url.path().starts_with("//") && path.starts_with("//") {
                spec = format!("{}:{path}", parsed.scheme());
                if let Some(value) = &query { spec.push('?'); spec.push_str(value); }
                if let Some(value) = parsed.fragment() { spec.push('#'); spec.push_str(value); }
            }
        }
    }
    let mut output = vec![1, u8::from(!blob_file && (blob_nested || matches!(origin, Origin::Opaque(_))))];
    for field in [Some(spec.as_str()), Some(parsed.scheme()), Some(username.as_str()),
                  password.as_deref(), host, port.as_deref(), Some(path.as_str()), query.as_deref(),
                  parsed.fragment(), Some(serialized_origin.as_str()), domain] {
        put_field(&mut output, field);
    }
    output.push(match same { Some(true) => 1, Some(false) => 0, None => 2 });
    Ok(output)
}

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    if &header != b"MGB1" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid protocol header"));
    }
    output.write_all(b"MGB1")?;
    output.flush()?;
    loop {
        let count = input.read(&mut header[..1])?;
        if count == 0 { break; }
        input.read_exact(&mut header[1..])?;
        let mut request = vec![0; u32::from_be_bytes(header) as usize];
        input.read_exact(&mut request)?;
        let response = respond(&request)?;
        output.write_all(&(response.len() as u32).to_be_bytes())?;
        output.write_all(&response)?;
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{put_field, registrable_domain, respond, take_field};

    #[test]
    fn registrable_domains_follow_chromium_suffix_data() {
        for (host, private, expected) in [
            ("www.example.co.uk", false, Some("example.co.uk")),
            ("a.blogspot.com", true, Some("a.blogspot.com")),
            ("a.blogspot.com", false, Some("blogspot.com")),
            ("a.b.ck", true, Some("a.b.ck")),
            ("www.ck", true, Some("www.ck")),
            ("127.0.0.1", true, None),
            ("127.1", true, None),
            ("[::1]", true, None),
            ("..www.example.com.", true, Some("example.com.")),
            ("...", true, None),
        ] {
            assert_eq!(registrable_domain(host, private), expected, "{host}");
        }
    }

    #[test]
    fn resolves_relative_url_and_reports_private_domain() {
        let mut request = Vec::new();
        put_field(&mut request, Some("../two?q=1#f"));
        put_field(&mut request, Some("https://a.blogspot.com/one/"));
        put_field(&mut request, Some("https://b.blogspot.com/"));
        request.push(1);
        let response = respond(&request).unwrap();
        assert_eq!(&response[..2], &[1, 0]);
        let mut fields = &response[2..];
        let values: Vec<_> = (0..11).map(|_| take_field(&mut fields).unwrap()).collect();
        assert_eq!(values[0], Some("https://a.blogspot.com/two?q=1#f"));
        assert_eq!(values[9], Some("https://a.blogspot.com"));
        assert_eq!(values[10], Some("a.blogspot.com"));
        assert_eq!(fields, &[0]);
    }

    #[test]
    fn file_origin_matches_chromium() {
        let mut request = Vec::new();
        put_field(&mut request, Some("file:///tmp/a"));
        put_field(&mut request, None);
        put_field(&mut request, None);
        request.push(0);
        let response = respond(&request).unwrap();
        assert_eq!(&response[..2], &[1, 0]);
        let mut fields = &response[2..];
        let values: Vec<_> = (0..11).map(|_| take_field(&mut fields).unwrap()).collect();
        assert_eq!(values[9], Some("file://"));
    }

    #[test]
    fn wpt_974_opaque_android_scheme_keeps_authority_in_path() {
        let mut request = Vec::new();
        put_field(&mut request, Some("android://x:0/a"));
        put_field(&mut request, None);
        put_field(&mut request, None);
        request.push(0);
        let response = respond(&request).unwrap();
        let mut fields = &response[2..];
        let values: Vec<_> = (0..11).map(|_| take_field(&mut fields).unwrap()).collect();
        assert_eq!(values[4], None);
        assert_eq!(values[5], None);
        assert_eq!(values[6], Some("//x:0/a"));
    }

    fn spec(input: &str, base: Option<&str>) -> String {
        let mut request = Vec::new();
        put_field(&mut request, Some(input));
        put_field(&mut request, base);
        put_field(&mut request, None);
        request.push(0);
        let response = respond(&request).unwrap();
        assert_eq!(response[0], 1);
        take_field(&mut &response[2..]).unwrap().unwrap().to_owned()
    }

    #[test]
    fn wpt_1008_non_special_single_backslash_keeps_host() {
        assert_eq!(spec("\\a", Some("foo://foo/a")), "foo://foo/a");
    }

    #[test]
    fn wpt_1009_non_special_two_slashes_become_path() {
        assert_eq!(spec("\\/a", Some("foo://foo/a")), "foo:/\\/a");
    }

    #[test]
    fn wpt_1017_empty_non_special_base_path_retains_spec_prefix() {
        assert_eq!(spec("?q2", Some("git://host?q#r")), "git://host?q#r?q2");
    }

    #[test]
    fn wpt_769_non_special_double_slash_path_stays_unambiguous() {
        assert_eq!(spec("path", Some("non-spec:/..//p")), "non-spec:/.//path");
    }

    #[test]
    fn wpt_853_userinfo_apostrophe_is_escaped() {
        assert_eq!(spec("wss://a'b@host/", None), "wss://a%27b@host/");
    }

    #[test]
    fn schemeful_site_compares_scheme_and_private_suffix() {
        for (first, second, expected) in [
            ("https://a.example.com/", "https://b.example.com/", 1),
            ("https://example.com/", "http://example.com/", 0),
            ("https://a.blogspot.com/", "https://b.blogspot.com/", 0),
            ("data:text/plain,a", "data:text/plain,a", 0),
        ] {
            let mut request = Vec::new();
            put_field(&mut request, Some(first));
            put_field(&mut request, None);
            put_field(&mut request, Some(second));
            request.push(0);
            put_field(&mut request, Some("schemeful-site"));
            put_field(&mut request, Some(""));
            assert_eq!(respond(&request).unwrap().last().copied(), Some(expected));
        }
    }

    #[test]
    fn setters_preserve_resolved_file_path_and_reject_invalid_input() {
        let mut request = Vec::new();
        put_field(&mut request, Some("..//a/../b"));
        put_field(&mut request, Some("file:///tmp/a"));
        put_field(&mut request, None);
        request.push(0);
        put_field(&mut request, Some("blink:href"));
        put_field(&mut request, Some("#fragment"));
        let response = respond(&request).unwrap();
        assert_eq!(take_field(&mut &response[2..]).unwrap(), Some("file:///.//b"));

        let mut request = Vec::new();
        put_field(&mut request, Some("..// space"));
        put_field(&mut request, Some("file:///tmp/a"));
        put_field(&mut request, None);
        request.push(1);
        put_field(&mut request, Some("host"));
        put_field(&mut request, Some("x@y"));
        let response = respond(&request).unwrap();
        assert_eq!(take_field(&mut &response[2..]).unwrap(), Some("file:///.//%20space"));

        let mut request = Vec::new();
        put_field(&mut request, Some("filesystem:/ space#"));
        put_field(&mut request, None);
        put_field(&mut request, None);
        request.push(0);
        put_field(&mut request, Some("protocol"));
        put_field(&mut request, Some("file:"));
        assert_eq!(respond(&request).unwrap(), [0]);
    }

    #[test]
    fn wpt_setter_pathname_23_retains_multiple_file_slashes() {
        let mut request = Vec::new();
        put_field(&mut request, Some("file:///unicorn"));
        put_field(&mut request, None);
        put_field(&mut request, None);
        request.push(0);
        put_field(&mut request, Some("blink:pathname"));
        put_field(&mut request, Some("//\\/"));
        let response = respond(&request).unwrap();
        assert_eq!(take_field(&mut &response[2..]).unwrap(), Some("file://////"));
    }

    #[test]
    fn wpt_setter_search_10_preserves_opaque_trailing_space() {
        let mut request = Vec::new();
        put_field(&mut request, Some("data:space ?query"));
        put_field(&mut request, None);
        put_field(&mut request, None);
        request.push(0);
        put_field(&mut request, Some("blink:search"));
        put_field(&mut request, Some(""));
        let response = respond(&request).unwrap();
        assert_eq!(take_field(&mut &response[2..]).unwrap(), Some("data:space "));
    }
}
