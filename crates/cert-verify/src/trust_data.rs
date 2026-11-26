use std::rc::Rc;

use crate::cert::ParsedCertificate;

const TRUST_DATA: &str = include_str!("../data/trust-data.txt");

#[derive(Clone, Debug, Default)]
pub struct RootConstraint {
    pub sct_not_after: Option<i64>,
    pub sct_all_after: Option<i64>,
    pub min_version: Option<Vec<u32>>,
    pub max_version_exclusive: Option<Vec<u32>>,
    pub permitted_dns_names: Vec<String>,
    pub validity_starts_not_after: Option<i64>,
    pub validity_starts_after: Option<i64>,
}

pub struct Anchor {
    pub cert: Rc<ParsedCertificate>,
    pub enforce_expiry: bool,
    pub enforce_constraints: bool,
    pub constraints: Vec<RootConstraint>,
}

pub struct TrustData {
    pub chrome_version: Vec<u32>,
    pub anchors: Vec<Anchor>,
    pub ct_log_list_micros: i64,
    pub blocked_spkis: Vec<[u8; 32]>,
    pub interception_spkis: Vec<[u8; 32]>,
}

fn version(text: &str) -> Vec<u32> {
    text.split('.').map(|part| part.parse().expect("trust data versions are dotted integers")).collect()
}

fn optional_micros(text: &str) -> Option<i64> {
    (text != "-").then(|| text.parse().expect("trust data times are integers"))
}

fn hash(text: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).expect("trust data hashes are hex");
    }
    out
}

fn base64(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    for c in text.bytes() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => panic!("trust data certificates are base64"),
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    out
}

pub fn load() -> TrustData {
    let mut data = TrustData { chrome_version: Vec::new(), anchors: Vec::new(), ct_log_list_micros: 0, blocked_spkis: Vec::new(), interception_spkis: Vec::new() };
    for line in TRUST_DATA.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        match fields[0] {
            "chrome-version" => data.chrome_version = version(fields[1]),
            "anchor" => {
                let der = base64(fields[1]);
                let cert = ParsedCertificate::parse(&der).expect("Chrome Root Store certificates parse");
                data.anchors.push(Anchor { cert: Rc::new(cert), enforce_expiry: fields[3] == "1", enforce_constraints: fields[5] == "1", constraints: Vec::new() });
            }
            "constraint" => {
                let value = |key: &str| fields.iter().position(|f| *f == key).map(|i| fields[i + 1]).expect("constraint fields are complete");
                let constraint = RootConstraint {
                    sct_not_after: optional_micros(value("sct-not-after")),
                    sct_all_after: optional_micros(value("sct-all-after")),
                    min_version: (value("min-version") != "-").then(|| version(value("min-version"))),
                    max_version_exclusive: (value("max-version-exclusive") != "-").then(|| version(value("max-version-exclusive"))),
                    permitted_dns_names: match value("dns") {
                        "-" => Vec::new(),
                        names => names.split(',').map(str::to_owned).collect(),
                    },
                    validity_starts_not_after: optional_micros(value("validity-starts-not-after")),
                    validity_starts_after: optional_micros(value("validity-starts-after")),
                };
                data.anchors.last_mut().expect("constraints follow their anchor").constraints.push(constraint);
            }
            "ct-log-list" => data.ct_log_list_micros = optional_micros(fields[1]).expect("the CT log list has a timestamp"),
            "blocked-spki" => data.blocked_spkis.push(hash(fields[1])),
            "interception-spki" => data.interception_spkis.push(hash(fields[1])),
            "ev" => assert!(
                fields[2].split('.').filter(|arc| arc.parse::<u64>().is_ok()).count() < 2,
                "the trust data lists an EV policy, but this build's EV metadata is empty and EV verification is not modelled"
            ),
            "crs-version" => {}
            other => panic!("unknown trust data line {other}"),
        }
    }
    data
}
