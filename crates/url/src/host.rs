use url_parser::Url;

fn unescape(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn canonical_host(input: &str) -> Option<String> {
    let decoded = unescape(input)?;
    let ascii = if decoded.is_ascii() { decoded } else { idna::domain_to_ascii(&decoded).ok()? };
    let ascii = if ascii.contains('%') { unescape(&ascii)? } else { ascii };
    if ascii.starts_with('[') && ascii.ends_with(']') {
        let address = &ascii[1..ascii.len() - 1];
        if let Ok(ip) = address.parse::<std::net::Ipv6Addr>() { return Some(format!("[{ip}]")); }
        if let Some(index) = address.rfind(':') {
            let ipv4 = &address[index + 1..];
            let mut parts = Vec::new();
            for part in ipv4.split('.') { parts.push(part.parse::<u8>().ok()?.to_string()); }
            if parts.len() == 4 {
                let value = format!("{}:{}", &address[..index], parts.join("."));
                if let Ok(ip) = value.parse::<std::net::Ipv6Addr>() { return Some(format!("[{ip}]")); }
            }
        }
        return None;
    }
    if ascii.is_empty() { return None; }
    let mut output = String::new();
    for byte in ascii.bytes() {
        match byte {
            b' ' => output.push_str("%20"),
            b'*' => output.push_str("%2A"),
            0..=31 | 127..=255 | b'#' | b'%' | b'/' | b'<' | b'>' | b'?' | b'@' | b'\\' | b'^' | b'|' | b'[' | b']' | b':' => return None,
            _ => output.push((byte as char).to_ascii_lowercase()),
        }
    }
    Some(output)
}

pub fn parse(input: &str) -> Option<(Url, String)> {
    let scheme_end = input.find(':')?;
    let scheme = &input[..scheme_end];
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "ftp" | "ws" | "wss") { return None; }
    let rest = input[scheme_end + 1..].trim_start_matches(['/', '\\']);
    let normalized_input = format!("{scheme}://{rest}");
    let input = normalized_input.as_str();
    let start = scheme_end + 3;
    let end = start + input[start..].find(['/', '\\', '?', '#']).unwrap_or(input.len() - start);
    let authority = &input[start..end];
    let host_start = authority.rfind('@').map_or(0, |i| i + 1);
    let hostport = &authority[host_start..];
    let decoded = unescape(hostport)?;
    let host_end = if decoded.starts_with('[') {
        let index = hostport.rfind(']')?;
        index + 1
    } else {
        hostport.rfind(':').unwrap_or(hostport.len())
    };
    let host = canonical_host(&hostport[..host_end])?;
    if host.starts_with('[') {
        let normalized = format!("{}{}{}{}", &input[..start + host_start], host, &hostport[host_end..], &input[end..]);
        let url = Url::parse(&normalized).ok()?;
        return Some((url, host));
    }
    let last = host.trim_end_matches('.').rsplit('.').next()?;
    let number = !last.is_empty() && (last.bytes().all(|b| b.is_ascii_digit()) ||
        last.strip_prefix("0x").is_some_and(|tail| tail.bytes().all(|b| b.is_ascii_hexdigit())));
    if number && Url::parse(&format!("http://{host}/")).is_err() { return None; }
    let normalized = format!("{}manganese.invalid{}{}", &input[..start + host_start], &hostport[host_end..], &input[end..]);
    let url = Url::parse(&normalized).ok()?;
    Some((url, host))
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn wpt_281_space_in_special_host() {
        assert_eq!(parse("http://example example.com").unwrap().1, "example%20example.com");
    }

    #[test]
    fn wpt_314_ascii_punycode_is_not_revalidated() {
        assert_eq!(parse("http://a.b.c.xn--pokxncvks").unwrap().1, "a.b.c.xn--pokxncvks");
    }

    #[test]
    fn wpt_364_escaped_ipv6_open_bracket() {
        assert_eq!(parse("http://%5B::1]").unwrap().1, "[::1]");
    }

    #[test]
    fn wpt_341_rejects_invalid_ipv4() {
        assert!(parse("http://192.168.0.257").is_none());
    }

    #[test]
    fn special_host_after_single_slash() {
        assert_eq!(parse("http:/ space#").unwrap().1, "%20space");
    }
}
