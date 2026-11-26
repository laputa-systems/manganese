use crate::der;
use crate::der::Parser;
use crate::errors::CertError;
use crate::errors::CertErrors;

const OID_EMAIL_ADDRESS: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x01];

pub const NAME_OTHER: u32 = 1 << 0;
pub const NAME_RFC822: u32 = 1 << 1;
pub const NAME_DNS: u32 = 1 << 2;
pub const NAME_X400: u32 = 1 << 3;
pub const NAME_DIRECTORY: u32 = 1 << 4;
pub const NAME_EDI_PARTY: u32 = 1 << 5;
pub const NAME_URI: u32 = 1 << 6;
pub const NAME_IP: u32 = 1 << 7;
pub const NAME_REGISTERED_ID: u32 = 1 << 8;
const NAME_ALL: u32 = (1 << 9) - 1;
const SUPPORTED_CONSTRAINT_TYPES: u32 = NAME_RFC822 | NAME_DNS | NAME_DIRECTORY | NAME_IP;

pub struct Attribute<'a> {
    pub oid: &'a [u8],
    pub tag: der::Tag,
    pub value: &'a [u8],
}

pub fn read_rdn<'a>(parser: &mut Parser<'a>) -> Option<Vec<Attribute<'a>>> {
    let mut out = Vec::new();
    while parser.has_more() {
        let mut attribute = parser.read_sequence()?;
        let oid = attribute.read_tag(der::OBJECT)?;
        let (tag, value) = attribute.read_tag_and_value()?;
        if attribute.has_more() {
            return None;
        }
        out.push(Attribute { oid, tag, value });
    }
    (!out.is_empty()).then_some(out)
}

pub fn parse_name_value(name_value: &[u8]) -> Option<Vec<Vec<Attribute<'_>>>> {
    let mut parser = Parser::new(name_value);
    let mut rdns = Vec::new();
    while parser.has_more() {
        let mut rdn = parser.read_constructed(der::SET)?;
        rdns.push(read_rdn(&mut rdn)?);
    }
    Some(rdns)
}

impl Attribute<'_> {
    pub fn value_as_string(&self) -> Option<Vec<u8>> {
        match self.tag {
            der::T61_STRING => Some(der::parse_teletex_as_latin1(self.value)),
            der::IA5_STRING => self.value.iter().all(|c| *c <= 127).then(|| self.value.to_vec()),
            der::PRINTABLE_STRING => der::is_printable_string(self.value).then(|| self.value.to_vec()),
            der::UTF8_STRING => Some(self.value.to_vec()),
            der::UNIVERSAL_STRING => der::parse_universal_string(self.value),
            der::BMP_STRING => der::parse_bmp_string(self.value),
            _ => None,
        }
    }

    fn value_as_string_unsafe(&self) -> Option<Vec<u8>> {
        match self.tag {
            der::IA5_STRING | der::PRINTABLE_STRING | der::T61_STRING | der::UTF8_STRING => Some(self.value.to_vec()),
            der::UNIVERSAL_STRING => der::parse_universal_string(self.value),
            der::BMP_STRING => der::parse_bmp_string(self.value),
            _ => None,
        }
    }

    fn is_normalizable(&self) -> bool {
        matches!(self.tag, der::PRINTABLE_STRING | der::UTF8_STRING | der::IA5_STRING | der::UNIVERSAL_STRING | der::BMP_STRING)
    }

    fn normalized(&self) -> Option<Vec<u8>> {
        let value = self.value_as_string_unsafe()?;
        let enforcement = match self.tag {
            der::PRINTABLE_STRING => Charset::Printable,
            der::BMP_STRING | der::UNIVERSAL_STRING | der::UTF8_STRING => Charset::Any,
            der::IA5_STRING => Charset::Ascii,
            _ => return None,
        };
        normalize_directory_string(&value, enforcement)
    }
}

#[derive(Clone, Copy)]
enum Charset {
    Any,
    Printable,
    Ascii,
}

fn normalize_directory_string(input: &[u8], charset: Charset) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len());
    let start = input.iter().take_while(|c| **c == b' ').count();
    let input = &input[start..];
    for (index, &c) in input.iter().enumerate() {
        if c == b' ' {
            if let Some(&next) = input.get(index + 1) {
                if next != b' ' {
                    out.push(b' ');
                }
            }
        } else if c.is_ascii_uppercase() {
            out.push(c + (b'a' - b'A'));
        } else {
            let allowed = match charset {
                Charset::Printable => c.is_ascii_lowercase() || (b'\''..=b':').contains(&c) || c == b'=' || c == b'?',
                Charset::Ascii => c <= 0x7f,
                Charset::Any => true,
            };
            if !allowed {
                return None;
            }
            out.push(c);
        }
    }
    Some(out)
}

fn value_match(a: &Attribute<'_>, b: &Attribute<'_>) -> bool {
    if a.is_normalizable() && b.is_normalizable() {
        return match (a.normalized(), b.normalized()) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        };
    }
    a.tag == b.tag && a.value == b.value
}

fn rdn_match(a: &mut Parser<'_>, b: &mut Parser<'_>) -> bool {
    let (Some(a_values), Some(mut b_values)) = (read_rdn(a), read_rdn(b)) else { return false };
    if a_values.len() != b_values.len() {
        return false;
    }
    for a_value in &a_values {
        match b_values.iter().position(|b_value| a_value.oid == b_value.oid && value_match(a_value, b_value)) {
            Some(index) => {
                b_values.remove(index);
            }
            None => return false,
        }
    }
    true
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NameMatch {
    Exact,
    Subtree,
}

fn name_match(a: &[u8], b: &[u8], kind: NameMatch) -> bool {
    let mut a_count = Parser::new(a);
    let mut b_count = Parser::new(b);
    while a_count.has_more() && b_count.has_more() {
        if !a_count.skip_tag(der::SET) || !b_count.skip_tag(der::SET) {
            return false;
        }
    }
    if b_count.has_more() || (kind == NameMatch::Exact && a_count.has_more()) {
        return false;
    }
    let mut a_rdns = Parser::new(a);
    let mut b_rdns = Parser::new(b);
    while a_rdns.has_more() && b_rdns.has_more() {
        let (Some(mut a_rdn), Some(mut b_rdn)) = (a_rdns.read_constructed(der::SET), b_rdns.read_constructed(der::SET)) else {
            return false;
        };
        if !rdn_match(&mut a_rdn, &mut b_rdn) {
            return false;
        }
    }
    true
}

pub fn verify_name_in_subtree(name: &[u8], parent: &[u8]) -> bool {
    name_match(name, parent, NameMatch::Subtree)
}

fn sort_set_elements(elements: &mut [Vec<u8>]) {
    elements.sort_by(|a, b| {
        let shared = a.len().min(b.len());
        a[..shared].cmp(&b[..shared]).then(a.len().cmp(&b.len()))
    });
}

pub fn normalize_name(name_value: &[u8]) -> Option<Vec<u8>> {
    let mut parser = Parser::new(name_value);
    let mut out = Vec::new();
    while parser.has_more() {
        let mut rdn_parser = parser.read_constructed(der::SET)?;
        let attributes = read_rdn(&mut rdn_parser)?;
        let mut elements = Vec::new();
        for attribute in &attributes {
            let mut content = der::encode(der::OBJECT, attribute.oid);
            if attribute.is_normalizable() {
                content.extend(der::encode(der::UTF8_STRING, &attribute.normalized()?));
            } else {
                content.extend(der::encode(attribute.tag, attribute.value));
            }
            elements.push(der::encode(der::SEQUENCE, &content));
        }
        sort_set_elements(&mut elements);
        out.extend(der::encode(der::SET, &elements.concat()));
    }
    Some(out)
}

fn find_email_addresses(name_value: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut parser = Parser::new(name_value);
    let mut out = Vec::new();
    while parser.has_more() {
        let mut rdn = parser.read_constructed(der::SET)?;
        for attribute in read_rdn(&mut rdn)? {
            if attribute.oid == OID_EMAIL_ADDRESS {
                out.push(attribute.value_as_string()?);
            }
        }
    }
    Some(out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IpForm {
    Address,
    AddressAndNetmask,
}

#[derive(Clone, Debug, Default)]
pub struct GeneralNames {
    pub present: u32,
    pub rfc822: Vec<Vec<u8>>,
    pub dns: Vec<Vec<u8>>,
    pub directory: Vec<Vec<u8>>,
    pub uri: Vec<Vec<u8>>,
    pub ip: Vec<Vec<u8>>,
    pub ip_ranges: Vec<(Vec<u8>, Vec<u8>)>,
}

pub fn is_valid_netmask(mask: &[u8]) -> bool {
    if mask.len() != 4 && mask.len() != 16 {
        return false;
    }
    for (index, &byte) in mask.iter().enumerate() {
        if byte != 0xff {
            let inverse = !byte;
            if inverse & inverse.wrapping_add(1) != 0 {
                return false;
            }
            return mask[index + 1..].iter().all(|b| *b == 0);
        }
    }
    true
}

pub fn ip_matches(address: &[u8], network: &[u8], mask: &[u8]) -> bool {
    address.len() == network.len() && address.len() == mask.len() && address.iter().zip(network).zip(mask).all(|((a, n), m)| a & m == n & m)
}

pub fn parse_general_name(input: &[u8], form: IpForm, names: &mut GeneralNames) -> bool {
    let mut parser = Parser::new(input);
    let Some((tag, value)) = parser.read_tag_and_value() else { return false };
    let kind = if tag == der::context_constructed(0) {
        NAME_OTHER
    } else if tag == der::context(1) {
        if !value.is_ascii() {
            return false;
        }
        names.rfc822.push(value.to_vec());
        NAME_RFC822
    } else if tag == der::context(2) {
        if !value.is_ascii() {
            return false;
        }
        names.dns.push(value.to_vec());
        NAME_DNS
    } else if tag == der::context_constructed(3) {
        NAME_X400
    } else if tag == der::context_constructed(4) {
        let mut name_parser = Parser::new(value);
        let Some(name_value) = name_parser.read_tag(der::SEQUENCE) else { return false };
        if name_parser.has_more() {
            return false;
        }
        names.directory.push(name_value.to_vec());
        NAME_DIRECTORY
    } else if tag == der::context_constructed(5) {
        NAME_EDI_PARTY
    } else if tag == der::context(6) {
        if !value.is_ascii() {
            return false;
        }
        names.uri.push(value.to_vec());
        NAME_URI
    } else if tag == der::context(7) {
        match form {
            IpForm::Address => {
                if value.len() != 4 && value.len() != 16 {
                    return false;
                }
                names.ip.push(value.to_vec());
            }
            IpForm::AddressAndNetmask => {
                if value.len() != 8 && value.len() != 32 {
                    return false;
                }
                let (address, mask) = value.split_at(value.len() / 2);
                if !is_valid_netmask(mask) {
                    return false;
                }
                names.ip_ranges.push((address.to_vec(), mask.to_vec()));
            }
        }
        NAME_IP
    } else if tag == der::context(8) {
        NAME_REGISTERED_ID
    } else {
        return false;
    };
    names.present |= kind;
    true
}

impl GeneralNames {
    pub fn parse(tlv: &[u8]) -> Option<Self> {
        let mut parser = Parser::new(tlv);
        let value = parser.read_tag(der::SEQUENCE)?;
        if parser.has_more() {
            return None;
        }
        Self::parse_value(value)
    }

    pub fn parse_value(value: &[u8]) -> Option<Self> {
        let mut names = Self::default();
        let mut parser = Parser::new(value);
        if !parser.has_more() {
            return None;
        }
        while parser.has_more() {
            let raw = parser.read_raw_tlv()?;
            if !parse_general_name(raw, IpForm::Address, &mut names) {
                return None;
            }
        }
        Some(names)
    }
}

fn ends_with_no_case(name: &[u8], suffix: &[u8]) -> bool {
    name.len() >= suffix.len() && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Wildcard {
    Partial,
    Full,
}

fn dns_name_matches(name: &[u8], constraint: &[u8], wildcard: Wildcard) -> bool {
    if constraint.is_empty() {
        return true;
    }
    let name = name.strip_suffix(b".").unwrap_or(name);
    let constraint = constraint.strip_suffix(b".").unwrap_or(constraint);
    if wildcard == Wildcard::Partial && name.len() > 2 && name[0] == b'*' && name[1] == b'.' {
        if let Some(dot) = constraint.iter().position(|c| *c == b'.') {
            if name[2..].eq_ignore_ascii_case(&constraint[dot + 1..]) {
                return true;
            }
        }
    }
    if !ends_with_no_case(name, constraint) {
        return false;
    }
    if name.len() == constraint.len() {
        return true;
    }
    let constraint = constraint.strip_prefix(b".").unwrap_or(constraint);
    name.len() > constraint.len() && name[name.len() - constraint.len() - 1] == b'.'
}

fn is_allowed_local_part(local: &[u8]) -> bool {
    !local.is_empty() && local.iter().all(|&c| c.is_ascii_alphanumeric() || b"!#$%&'*+-/=?^_`{|}~.".contains(&c))
}

fn is_allowed_domain(domain: &[u8]) -> bool {
    !domain.is_empty() && domain.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.')
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rfc822Match {
    Permitted,
    Excluded,
}

fn rfc822_matches(local: &[u8], domain: &[u8], constraint: &[u8], kind: Rfc822Match, case_insensitive_local: bool) -> bool {
    let error_value = kind == Rfc822Match::Excluded;
    let parts: Vec<&[u8]> = constraint.split(|c| *c == b'@').collect();
    let (constraint_local, constraint_domain): (&[u8], &[u8]) = match parts.as_slice() {
        [domain] => (b"", domain),
        [local, domain] => {
            if !is_allowed_local_part(local) {
                return error_value;
            }
            (local, domain)
        }
        _ => return error_value,
    };
    if !is_allowed_domain(constraint_domain) {
        return error_value;
    }
    if !constraint_local.is_empty() {
        let local_equal = if case_insensitive_local { local.eq_ignore_ascii_case(constraint_local) } else { local == constraint_local };
        return local_equal && domain.eq_ignore_ascii_case(constraint_domain);
    }
    if constraint_domain.first() == Some(&b'.') {
        return ends_with_no_case(domain, constraint_domain);
    }
    domain.eq_ignore_ascii_case(constraint_domain)
}

#[derive(Clone, Debug, Default)]
pub struct NameConstraints {
    permitted: GeneralNames,
    excluded: GeneralNames,
    constrained_types: u32,
}

fn parse_subtrees(value: &[u8], names: &mut GeneralNames) -> bool {
    let mut parser = Parser::new(value);
    if !parser.has_more() {
        return false;
    }
    while parser.has_more() {
        let Some(mut subtree) = parser.read_sequence() else { return false };
        let Some(raw) = subtree.read_raw_tlv() else { return false };
        if !parse_general_name(raw, IpForm::AddressAndNetmask, names) || subtree.has_more() {
            return false;
        }
    }
    true
}

impl NameConstraints {
    pub fn parse(value: &[u8], critical: bool) -> Option<Self> {
        let mut outer = Parser::new(value);
        let mut sequence = outer.read_sequence()?;
        if outer.has_more() {
            return None;
        }
        let mut out = Self::default();
        let mask = if critical { NAME_ALL } else { SUPPORTED_CONSTRAINT_TYPES };
        let permitted = sequence.read_optional_tag(der::context_constructed(0))?;
        if let Some(permitted) = permitted {
            if !parse_subtrees(permitted, &mut out.permitted) {
                return None;
            }
        }
        out.constrained_types |= out.permitted.present & mask;
        let excluded = sequence.read_optional_tag(der::context_constructed(1))?;
        if let Some(excluded) = excluded {
            if !parse_subtrees(excluded, &mut out.excluded) {
                return None;
            }
        }
        out.constrained_types |= out.excluded.present & mask;
        if permitted.is_none() && excluded.is_none() {
            return None;
        }
        if sequence.has_more() {
            return None;
        }
        Some(out)
    }

    pub fn from_permitted(permitted: GeneralNames) -> Self {
        Self { constrained_types: permitted.present, permitted, excluded: GeneralNames::default() }
    }

    pub fn check(&self, subject: &[u8], alt_names: Option<&GeneralNames>, errors: &mut CertErrors) {
        const MAX_CHECKS: usize = 1 << 20;
        let (names, constraints) = match alt_names {
            Some(alt) => (
                alt.rfc822.len() + alt.dns.len() + alt.directory.len() + alt.ip.len(),
                self.excluded.rfc822.len()
                    + self.permitted.rfc822.len()
                    + self.excluded.dns.len()
                    + self.permitted.dns.len()
                    + self.excluded.directory.len()
                    + self.permitted.directory.len()
                    + self.excluded.ip_ranges.len()
                    + self.permitted.ip_ranges.len(),
            ),
            None => (subject.len(), self.excluded.directory.len() + self.permitted.directory.len()),
        };
        match constraints.checked_mul(names) {
            Some(count) if count <= MAX_CHECKS => {}
            _ => {
                errors.add_error(CertError::TooManyNameConstraintChecks);
                return;
            }
        }
        let mut subject_emails = Vec::new();
        if alt_names.is_none() && self.constrained_types & NAME_RFC822 != 0 {
            match find_email_addresses(subject) {
                Some(emails) => subject_emails = emails,
                None => {
                    errors.add_error(CertError::NotPermittedByNameConstraints);
                    return;
                }
            }
        }
        if let Some(alt) = alt_names {
            if self.constrained_types & alt.present & !SUPPORTED_CONSTRAINT_TYPES != 0 {
                errors.add_error(CertError::NotPermittedByNameConstraints);
                return;
            }
            let permitted = (self.constrained_types & NAME_RFC822 == 0 || alt.rfc822.iter().all(|n| self.permits_rfc822(n, false)))
                && alt.dns.iter().all(|n| self.permits_dns(n))
                && alt.directory.iter().all(|n| self.permits_directory(n))
                && alt.ip.iter().all(|n| self.permits_ip(n));
            if !permitted {
                errors.add_error(CertError::NotPermittedByNameConstraints);
                return;
            }
        }
        if !subject_emails.iter().all(|n| self.permits_rfc822(n, true)) {
            errors.add_error(CertError::NotPermittedByNameConstraints);
            return;
        }
        if alt_names.is_some() && subject.is_empty() {
            return;
        }
        if !self.permits_directory(subject) {
            errors.add_error(CertError::NotPermittedByNameConstraints);
        }
    }

    fn permits_rfc822(&self, name: &[u8], case_insensitive_exclude_local: bool) -> bool {
        let parts: Vec<&[u8]> = name.split(|c| *c == b'@').collect();
        let [local, domain] = parts.as_slice() else { return false };
        if !is_allowed_local_part(local) || !is_allowed_domain(domain) {
            return false;
        }
        if self.excluded.rfc822.iter().any(|c| rfc822_matches(local, domain, c, Rfc822Match::Excluded, case_insensitive_exclude_local)) {
            return false;
        }
        if self.permitted.present & NAME_RFC822 == 0 {
            return true;
        }
        self.permitted.rfc822.iter().any(|c| rfc822_matches(local, domain, c, Rfc822Match::Permitted, false))
    }

    fn permits_dns(&self, name: &[u8]) -> bool {
        if self.excluded.dns.iter().any(|c| dns_name_matches(name, c, Wildcard::Partial)) {
            return false;
        }
        if self.permitted.present & NAME_DNS == 0 {
            return true;
        }
        self.permitted.dns.iter().any(|c| dns_name_matches(name, c, Wildcard::Full))
    }

    fn permits_directory(&self, name: &[u8]) -> bool {
        if self.excluded.directory.iter().any(|c| verify_name_in_subtree(name, c)) {
            return false;
        }
        if self.permitted.present & NAME_DIRECTORY == 0 {
            return true;
        }
        self.permitted.directory.iter().any(|c| verify_name_in_subtree(name, c))
    }

    fn permits_ip(&self, address: &[u8]) -> bool {
        if self.excluded.ip_ranges.iter().any(|(network, mask)| ip_matches(address, network, mask)) {
            return false;
        }
        if self.permitted.present & NAME_IP == 0 {
            return true;
        }
        self.permitted.ip_ranges.iter().any(|(network, mask)| ip_matches(address, network, mask))
    }
}
