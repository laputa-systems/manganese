use publicsuffix::{IcannList, List, Psl};
use std::io::{self, Read, Write};
use std::sync::OnceLock;
use url_parser::{Origin, Url};

mod file_url;

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
    if host.is_empty() || ip.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
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

fn parse(input: &str, base: Option<&str>) -> Result<Url, url_parser::ParseError> {
    match base {
        Some(base) => Url::parse(base)?.join(input),
        None => Url::parse(input),
    }
}

fn respond(mut request: &[u8]) -> io::Result<Vec<u8>> {
    let input = take_field(&mut request)?.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing URL"))?;
    let base = take_field(&mut request)?;
    let other = take_field(&mut request)?;
    if request.len() != 1 || request[0] > 1 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid registry policy"));
    }
    let include_private = request[0] == 1;
    let trimmed = input.trim_matches(|c: char| c <= ' ');
    let file_scheme = trimmed.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:"));
    let input_scheme = trimmed.find(':').is_some_and(|i| {
        i > 0 && trimmed.as_bytes()[0].is_ascii_alphabetic() &&
        trimmed.as_bytes()[..i].iter().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(b))
    });
    let file_base = base.is_some_and(|s| s.trim_matches(|c: char| c <= ' ').get(..5).is_some_and(|p| p.eq_ignore_ascii_case("file:")));
    if file_scheme || (file_base && !input_scheme) {
        let file = match file_url::parse(input, if file_base { base } else { None }) {
            Some(file) => file,
            None => return Ok(vec![0]),
        };
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
        return Ok(output);
    }
    let parsed = match parse(input, base) {
        Ok(parsed) => parsed,
        Err(_) => return Ok(vec![0]),
    };
    let origin = parsed.origin();
    let is_file = parsed.scheme() == "file";
    let serialized_origin = if is_file { "file://".to_owned() } else { origin.ascii_serialization() };
    let port = parsed.port().map(|value| value.to_string());
    let host = parsed.host_str();
    let domain = host.and_then(|value| registrable_domain(value, include_private));
    let same = other.and_then(|value| Url::parse(value).ok()).and_then(|value| {
        let other_host = value.host_str()?;
        let this_host = host?;
        let a = domain.unwrap_or(this_host);
        let b = registrable_domain(other_host, include_private).unwrap_or(other_host);
        Some(a == b)
    });
    let mut output = vec![1, u8::from(!is_file && matches!(origin, Origin::Opaque(_)))];
    for field in [Some(parsed.as_str()), Some(parsed.scheme()), Some(parsed.username()),
                  parsed.password(), host, port.as_deref(), Some(parsed.path()), parsed.query(),
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
}
