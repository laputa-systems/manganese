const ABOUT_BLANK: &str = "about:blank";

fn is_c0_or_space(byte: u8) -> bool {
    byte <= b' '
}

fn clean(input: &str) -> String {
    let trimmed = input.trim_matches(|ch: char| ch.is_ascii() && is_c0_or_space(ch as u8));
    trimmed.chars().filter(|ch| !matches!(ch, '\t' | '\n' | '\r')).collect()
}

fn unescape(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            out.push(high << 4 | low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn forbidden_host_byte(byte: u8) -> bool {
    byte <= 0x20
        || byte == 0x7f
        || matches!(byte, b'#' | b'%' | b'/' | b':' | b'<' | b'>' | b'?' | b'@' | b'[' | b'\\' | b']' | b'^' | b'|')
}

#[derive(Debug, PartialEq)]
enum Ipv4 {
    Address,
    NotNumeric,
    Broken,
}

fn parse_ipv4_component(part: &str) -> Option<u64> {
    if part.is_empty() {
        return None;
    }
    let (digits, radix) = if let Some(rest) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
        (rest, 16)
    } else if part.len() > 1 && part.starts_with('0') {
        (&part[1..], 8)
    } else {
        (part, 10)
    };
    if digits.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(digits, radix).ok()
}

fn classify_ipv4(host: &str) -> Ipv4 {
    let mut parts: Vec<&str> = host.split('.').collect();
    if parts.len() > 1 && parts.last() == Some(&"") {
        parts.pop();
    }
    let last = parts.last().copied().unwrap_or_default();
    let numeric_last = !last.is_empty()
        && (last.bytes().all(|byte| byte.is_ascii_digit())
            || last.strip_prefix("0x").or_else(|| last.strip_prefix("0X")).is_some_and(|rest| rest.bytes().all(|byte| byte.is_ascii_hexdigit())));
    if !numeric_last {
        return Ipv4::NotNumeric;
    }
    if parts.len() > 4 {
        return Ipv4::Broken;
    }
    let mut values = Vec::with_capacity(parts.len());
    for part in &parts {
        match parse_ipv4_component(part) {
            Some(value) => values.push(value),
            None => return Ipv4::Broken,
        }
    }
    let (last_value, leading) = values.split_last().expect("at least one component");
    if leading.iter().any(|value| *value > 255) || *last_value >= 256u64.pow(5 - parts.len() as u32) {
        return Ipv4::Broken;
    }
    Ipv4::Address
}

fn ipv6_groups(part: &str, allow_ipv4_tail: bool) -> Option<usize> {
    if part.is_empty() {
        return Some(0);
    }
    let pieces: Vec<&str> = part.split(':').collect();
    let mut groups = 0;
    for (index, piece) in pieces.iter().enumerate() {
        if piece.contains('.') {
            if !allow_ipv4_tail || index + 1 != pieces.len() || piece.split('.').count() != 4 || classify_ipv4(piece) != Ipv4::Address {
                return None;
            }
            groups += 2;
        } else {
            if piece.is_empty() || piece.len() > 4 || !piece.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            groups += 1;
        }
    }
    Some(groups)
}

fn valid_ipv6(host: &str) -> bool {
    let Some(inner) = host.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) else {
        return false;
    };
    match inner.split_once("::") {
        Some((head, tail)) => match (ipv6_groups(head, false), ipv6_groups(tail, true)) {
            (Some(head), Some(tail)) => head + tail < 8,
            _ => false,
        },
        None => ipv6_groups(inner, true) == Some(8),
    }
}

fn is_local_hostname(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    host == "localhost" || host.ends_with(".localhost")
}

fn split_authority(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https:")?;
    let rest = rest.trim_start_matches(['/', '\\']);
    let end = rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len());
    Some(&rest[..end])
}

fn host_of(authority: &str) -> Option<&str> {
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    if hostport.starts_with('[') {
        let close = hostport.find(']')?;
        let after = &hostport[close + 1..];
        if !(after.is_empty() || after.strip_prefix(':').is_some_and(|port| port.bytes().all(|byte| byte.is_ascii_digit()) && port.parse::<u32>().is_ok_and(|port| port <= 65535) || port.is_empty())) {
            return None;
        }
        return Some(&hostport[..=close]);
    }
    match hostport.rfind(':') {
        Some(colon) => {
            let port = &hostport[colon + 1..];
            if !port.bytes().all(|byte| byte.is_ascii_digit()) || (!port.is_empty() && port.parse::<u32>().map_or(true, |port| port > 65535)) {
                return None;
            }
            Some(&hostport[..colon])
        }
        None => Some(hostport),
    }
}

pub fn host_is_safe(header: &str) -> bool {
    if header.is_empty() {
        return true;
    }
    let url = clean(&format!("https://{header}"));
    let Some(authority) = split_authority(&url) else {
        return false;
    };
    let Some(host) = host_of(authority) else {
        return false;
    };
    if host.starts_with('[') {
        return valid_ipv6(&host.to_ascii_lowercase());
    }
    let decoded = unescape(host).to_ascii_lowercase();
    if decoded.is_empty() || !decoded.is_ascii() || decoded.bytes().any(forbidden_host_byte) {
        return false;
    }
    match classify_ipv4(&decoded) {
        Ipv4::Address => true,
        Ipv4::Broken => false,
        Ipv4::NotNumeric => is_local_hostname(&decoded),
    }
}

fn standard_scheme(scheme: &str) -> bool {
    matches!(scheme, "http" | "https" | "ws" | "wss" | "ftp" | "file" | "chrome" | "chrome-untrusted" | "devtools" | "chrome-search")
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
    }
}

fn percent_escape_path(input: &str, out: &mut String, keep: &[char]) {
    for ch in input.chars() {
        if ch.is_ascii() && (ch as u8 <= b' ' || ch as u8 == 0x7f || matches!(ch, '"' | '<' | '>' | '`') && !keep.contains(&ch)) {
            out.push_str(&format!("%{:02X}", ch as u8));
        } else if !ch.is_ascii() {
            let mut buffer = [0u8; 4];
            for byte in ch.encode_utf8(&mut buffer).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(ch);
        }
    }
}

fn resolve_dots(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    let parts: Vec<&str> = path.split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        match part.to_ascii_lowercase().as_str() {
            "." | "%2e" => {
                if last {
                    segments.push("");
                }
            }
            ".." | ".%2e" | "%2e." | "%2e%2e" => {
                if segments.len() > 1 {
                    segments.pop();
                }
                if last {
                    segments.push("");
                }
            }
            _ => segments.push(part),
        }
    }
    segments.join("/")
}

fn canonical_standard(scheme: &str, rest: &str) -> Option<String> {
    let rest = rest.trim_start_matches(['/', '\\']);
    let end = rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host_part = host_of(authority)?;
    let host = if host_part.starts_with('[') {
        if !valid_ipv6(&host_part.to_ascii_lowercase()) {
            return None;
        }
        host_part.to_ascii_lowercase()
    } else {
        let decoded = unescape(host_part).to_ascii_lowercase();
        if decoded.is_empty() && scheme != "file" || !decoded.is_ascii() || decoded.bytes().any(forbidden_host_byte) {
            return None;
        }
        if classify_ipv4(&decoded) == Ipv4::Broken {
            return None;
        }
        decoded
    };
    let port = hostport.get(host_part.len()..).and_then(|rest| rest.strip_prefix(':')).filter(|port| !port.is_empty()).and_then(|port| port.parse::<u16>().ok());
    let tail = tail.replace('\\', "/");
    let (before_fragment, fragment) = match tail.split_once('#') {
        Some((before, fragment)) => (before.to_owned(), Some(fragment.to_owned())),
        None => (tail.clone(), None),
    };
    let (path, query) = match before_fragment.split_once('?') {
        Some((path, query)) => (path.to_owned(), Some(query.to_owned())),
        None => (before_fragment, None),
    };
    let path = if path.is_empty() { "/".to_owned() } else { resolve_dots(&path) };
    let mut out = format!("{scheme}://{host}");
    if let Some(port) = port.filter(|port| default_port(scheme) != Some(*port)) {
        out.push_str(&format!(":{port}"));
    }
    percent_escape_path(&path, &mut out, &[]);
    if let Some(query) = query {
        out.push('?');
        percent_escape_path(&query, &mut out, &['"', '<', '>']);
    }
    if let Some(fragment) = fragment {
        out.push('#');
        percent_escape_path(&fragment, &mut out, &['"', '<', '>', '`']);
    }
    Some(out)
}

pub fn target_url(escaped: &str) -> String {
    let decoded = clean(&unescape(escaped));
    let Some(colon) = decoded.find(':') else {
        return ABOUT_BLANK.to_owned();
    };
    let scheme = &decoded[..colon];
    let valid_scheme = scheme.chars().next().is_some_and(|ch| ch.is_ascii_alphabetic())
        && scheme.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'));
    if !valid_scheme {
        return ABOUT_BLANK.to_owned();
    }
    let scheme = scheme.to_ascii_lowercase();
    let rest = &decoded[colon + 1..];
    if standard_scheme(&scheme) {
        return canonical_standard(&scheme, rest).unwrap_or_else(|| ABOUT_BLANK.to_owned());
    }
    let mut out = format!("{scheme}:");
    let (before_fragment, fragment) = match rest.split_once('#') {
        Some((before, fragment)) => (before, Some(fragment)),
        None => (rest, None),
    };
    percent_escape_path(before_fragment, &mut out, &['"', '<', '>', '`']);
    if let Some(fragment) = fragment {
        out.push('#');
        percent_escape_path(fragment, &mut out, &['"', '<', '>', '`']);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_that_are_addresses_or_localhost_are_safe() {
        for host in ["", "127.0.0.1:9221", "localhost", "LOCALHOST", "localhost:7000", "[::1]:9221", "10.1.2.3", "8.8.8.8:80", "localhost.", "127.1", "a.localhost", "0x7f.1", "user@127.0.0.1"] {
            assert!(host_is_safe(host), "{host}");
        }
    }

    #[test]
    fn other_hosts_are_not_safe() {
        for host in ["relay.example:4444", "1.2.3.4.5", "256.1.1.1", "127.0.0.1:99999", "local host", "[::1", "127.0.0.x"] {
            assert!(!host_is_safe(host), "{host}");
        }
    }

    #[test]
    fn target_urls_are_canonicalised_or_replaced_by_about_blank() {
        assert_eq!(target_url("about:blank"), "about:blank");
        assert_eq!(target_url(""), "about:blank");
        assert_eq!(target_url("about%3Ablank"), "about:blank");
        assert_eq!(target_url("about:blank#x"), "about:blank#x");
        assert_eq!(target_url("http://example.test"), "http://example.test/");
        assert_eq!(target_url("chrome://version/"), "chrome://version/");
        assert_eq!(target_url("data:text/html,hi"), "data:text/html,hi");
        assert_eq!(target_url("nonsense"), "about:blank");
    }
}
