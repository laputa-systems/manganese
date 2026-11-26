use std::collections::HashMap;
use std::net::Ipv6Addr;
use std::sync::OnceLock;

const SUFFIX_DATA: &str = include_str!("../../url/src/effective_tld_names.dat");

#[derive(Clone, Copy, Default)]
struct Rule {
    exception: bool,
    wildcard: bool,
    private: bool,
}

fn rules() -> &'static HashMap<String, Rule> {
    static RULES: OnceLock<HashMap<String, Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let mut rules = HashMap::new();
        let mut extra: Vec<(String, bool)> = Vec::new();
        let mut private = false;
        for line in SUFFIX_DATA.lines() {
            if line.starts_with("// ===BEGIN PRIVATE DOMAINS===") {
                private = true;
                continue;
            }
            if line.starts_with("// ===END PRIVATE DOMAINS===") {
                private = false;
                continue;
            }
            if line.starts_with("//") || line.is_empty() {
                continue;
            }
            let line = line.split([' ', '\t', '\r']).next().unwrap_or_default();
            let mut domain = line.strip_prefix('.').unwrap_or(line);
            domain = domain.strip_suffix('.').unwrap_or(domain);
            let mut rule = Rule { private, ..Rule::default() };
            if let Some(rest) = domain.strip_prefix('!') {
                domain = rest;
                rule.exception = true;
            } else if let Some(rest) = domain.strip_prefix("*.") {
                domain = rest;
                rule.wildcard = true;
            }
            if domain.is_empty() {
                continue;
            }
            let Ok(normalized) = idna::domain_to_ascii(domain) else { continue };
            if let Some(dot) = normalized.rfind('.') {
                if dot + 1 < normalized.len() {
                    let tld = normalized[dot + 1..].to_owned();
                    let tld_private = private && extra.iter().rev().find(|(name, _)| *name == tld).is_none_or(|(_, p)| *p);
                    extra.retain(|(name, _)| *name != tld);
                    extra.push((tld, tld_private));
                }
            }
            rules.insert(normalized, rule);
        }
        for (tld, private) in extra {
            rules.entry(tld).or_insert(Rule { private, ..Rule::default() });
        }
        rules
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Unknown {
    Include,
    Exclude,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Private {
    Include,
    Exclude,
}

struct Registry<'a> {
    registry: &'a str,
    is_identifier: bool,
}

fn registry_in_trimmed(host: &str, unknown: Unknown, private: Private) -> Registry<'_> {
    let rules = rules();
    let mut found: Option<(Rule, usize)> = None;
    let bytes = host.as_bytes();
    for start in (0..bytes.len()).rev() {
        if start != 0 && bytes[start - 1] != b'.' {
            continue;
        }
        if let Some(rule) = rules.get(&host[start..]) {
            if rule.private && private == Private::Exclude {
                break;
            }
            found = Some((*rule, start));
        }
    }
    let Some((rule, start)) = found else {
        if unknown == Unknown::Include {
            if let Some(dot) = host.rfind('.') {
                return Registry { registry: &host[dot + 1..], is_identifier: false };
            }
        }
        return Registry { registry: "", is_identifier: false };
    };
    let length = host.len() - start;
    if rule.wildcard && !rule.exception {
        if length == host.len() {
            return Registry { registry: "", is_identifier: true };
        }
        return match host[..host.len() - length - 1].rfind('.') {
            Some(dot) => Registry { registry: &host[dot + 1..], is_identifier: false },
            None => Registry { registry: "", is_identifier: true },
        };
    }
    if rule.exception {
        let suffix = &host[start..];
        let dot = suffix.find('.').expect("exception rules have a parent label");
        return Registry { registry: &suffix[dot + 1..], is_identifier: false };
    }
    if length == host.len() {
        return Registry { registry: "", is_identifier: true };
    }
    Registry { registry: &host[start..], is_identifier: false }
}

fn registry(host: &str, unknown: Unknown, private: Private) -> Registry<'_> {
    let Some(begin) = host.find(|c| c != '.') else { return Registry { registry: "", is_identifier: false } };
    let mut trimmed = &host[begin..];
    trimmed = trimmed.strip_suffix('.').unwrap_or(trimmed);
    let result = registry_in_trimmed(trimmed, unknown, private);
    if result.registry.is_empty() {
        return result;
    }
    let offset = result.registry.as_ptr() as usize - host.as_ptr() as usize;
    Registry { registry: &host[offset..], is_identifier: result.is_identifier }
}

pub fn registry_length(host: &str, unknown: Unknown, private: Private) -> usize {
    registry(host, unknown, private).registry.len()
}

pub fn has_registry_controlled_domain(host: &str, unknown: Unknown, private: Private) -> bool {
    match parse_host(host) {
        Host::Name(name) => !registry(&name, unknown, private).registry.is_empty(),
        Host::Ip(_) | Host::Invalid => false,
    }
}

pub enum Host {
    Ip(Vec<u8>),
    Name(String),
    Invalid,
}

fn canonical_ipv4(host: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut out = [0u8; 4];
    for (slot, part) in out.iter_mut().zip(&parts) {
        if part.is_empty() || (part.len() > 1 && part.starts_with('0')) || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    Some(out)
}

fn looks_numeric(label: &str) -> bool {
    let hex = label.strip_prefix("0x").or_else(|| label.strip_prefix("0X"));
    match hex {
        Some(digits) => digits.bytes().all(|b| b.is_ascii_hexdigit()),
        None => !label.is_empty() && label.bytes().all(|b| b.is_ascii_digit()),
    }
}

pub fn parse_host(host: &str) -> Host {
    if host.contains(':') {
        return match host.parse::<Ipv6Addr>() {
            Ok(address) => Host::Ip(address.octets().to_vec()),
            Err(_) => Host::Invalid,
        };
    }
    if let Some(address) = canonical_ipv4(host) {
        return Host::Ip(address.to_vec());
    }
    let last = host.strip_suffix('.').unwrap_or(host).rsplit('.').next().unwrap_or_default();
    if looks_numeric(last) || host.is_empty() {
        return Host::Invalid;
    }
    Host::Name(host.to_ascii_lowercase())
}

fn alphanumeric(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit()
}

pub fn is_canonicalized_host_compliant(host: &str) -> bool {
    let bytes = host.as_bytes();
    if bytes.is_empty() || bytes.len() > 254 || (bytes[bytes.len() - 1] != b'.' && bytes.len() == 254) {
        return false;
    }
    let mut in_component = false;
    let mut started_alphanumeric = false;
    let mut label_size = 0usize;
    for &c in bytes {
        label_size += 1;
        if !in_component {
            started_alphanumeric = alphanumeric(c);
            if !started_alphanumeric && c != b'-' && c != b'_' {
                return false;
            }
            in_component = true;
        } else if c == b'.' {
            in_component = false;
            if label_size > 64 || label_size == 1 {
                return false;
            }
            label_size = 0;
        } else if !alphanumeric(c) && c != b'-' && c != b'_' {
            return false;
        }
    }
    label_size <= 63 && started_alphanumeric
}

fn split_on_dot(name: &str) -> (&str, &str) {
    match name.find('.') {
        Some(dot) => (&name[..dot], &name[dot..]),
        None => (name, ""),
    }
}

pub fn verify_hostname(hostname: &str, dns_names: &[Vec<u8>], ip_addresses: &[Vec<u8>]) -> bool {
    if dns_names.is_empty() && ip_addresses.is_empty() {
        return false;
    }
    let mut reference = match parse_host(hostname) {
        Host::Invalid => return false,
        Host::Ip(address) => return ip_addresses.contains(&address),
        Host::Name(name) => name,
    };
    if reference == "." || !is_canonicalized_host_compliant(&reference) {
        return false;
    }
    if reference.ends_with('.') {
        reference.pop();
    }
    let (reference_host, reference_domain) = split_on_dot(&reference);
    let mut allow_wildcards = false;
    if !reference_domain.is_empty() {
        let length = registry_length(&reference, Unknown::Include, Private::Exclude);
        let is_registry_controlled = length != 0 && length == reference_domain.len() - 1;
        allow_wildcards = !is_registry_controlled && reference.bytes().any(|c| !c.is_ascii_digit() && c != b'.');
    }
    for name in dns_names {
        if name.is_empty() || name.contains(&0) {
            continue;
        }
        let mut presented = name.to_ascii_lowercase();
        if presented.last() == Some(&b'.') {
            presented.pop();
        }
        if presented.len() > reference.len() {
            continue;
        }
        let Ok(presented) = std::str::from_utf8(&presented) else { continue };
        let (presented_host, presented_domain) = split_on_dot(presented);
        if presented_domain != reference_domain {
            continue;
        }
        if presented_host != "*" {
            if presented_host == reference_host {
                return true;
            }
            continue;
        }
        if allow_wildcards {
            return true;
        }
    }
    false
}

fn prefix_matches(address: &[u8], prefix: &[u8], bits: usize) -> bool {
    let full = bits / 8;
    if address[..full] != prefix[..full] {
        return false;
    }
    let rest = bits % 8;
    rest == 0 || (address[full] ^ prefix[full]) & (0xffu8 << (8 - rest)) == 0
}

fn ipv4_publicly_routable(address: &[u8]) -> bool {
    const RESERVED: [([u8; 4], usize); 14] = [
        ([0, 0, 0, 0], 8), ([10, 0, 0, 0], 8), ([100, 64, 0, 0], 10), ([127, 0, 0, 0], 8), ([169, 254, 0, 0], 16), ([172, 16, 0, 0], 12),
        ([192, 0, 0, 0], 24), ([192, 0, 2, 0], 24), ([192, 88, 99, 0], 24), ([192, 168, 0, 0], 16), ([198, 18, 0, 0], 15),
        ([198, 51, 100, 0], 24), ([203, 0, 113, 0], 24), ([224, 0, 0, 0], 3),
    ];
    !RESERVED.iter().any(|(prefix, bits)| prefix_matches(address, prefix, *bits))
}

pub fn is_publicly_routable(address: &[u8]) -> bool {
    if address.len() == 4 {
        return ipv4_publicly_routable(address);
    }
    if prefix_matches(address, &[0x20, 0], 3) || prefix_matches(address, &[0xff, 0], 8) {
        return true;
    }
    if address[..12] == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff] {
        return ipv4_publicly_routable(&address[12..]);
    }
    false
}

pub fn is_non_unique(hostname: &str) -> bool {
    match parse_host(hostname) {
        Host::Invalid => false,
        Host::Ip(address) => !is_publicly_routable(&address),
        Host::Name(name) => {
            let result = registry(&name, Unknown::Exclude, Private::Exclude);
            result.registry.is_empty() && !result.is_identifier
        }
    }
}
