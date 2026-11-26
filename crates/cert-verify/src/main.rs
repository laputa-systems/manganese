mod cert;
mod chain;
mod ct;
mod der;
mod errors;
mod hostname;
mod keys;
mod names;
mod path;
mod signature;
mod trust;
mod trust_data;
mod verify;

use std::io::Read;
use std::io::Write;

use path::CtCompliance;
use verify::Outcome;
use verify::TrustEntry;
use verify::TrustKind;
use verify::Verifier;

struct Request {
    time: i64,
    hostname: Vec<u8>,
    chain: Vec<Vec<u8>>,
    trust: Vec<TrustEntry>,
    scts: Vec<u8>,
    ocsp: Vec<u8>,
}

struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, size: usize) -> Option<&'a [u8]> {
        if self.data.len() < size {
            return None;
        }
        let (head, tail) = self.data.split_at(size);
        self.data = tail;
        Some(head)
    }

    fn uint(&mut self, width: usize) -> Option<u64> {
        Some(self.bytes(width)?.iter().fold(0u64, |value, byte| (value << 8) | u64::from(*byte)))
    }

    fn prefixed(&mut self, width: usize) -> Option<&'a [u8]> {
        let size = usize::try_from(self.uint(width)?).ok()?;
        self.bytes(size)
    }
}

fn parse_request(input: &[u8]) -> Result<Request, &'static str> {
    let mut reader = Reader { data: input };
    if reader.uint(1) != Some(1) {
        return Err("version");
    }
    let time = reader.uint(8).ok_or("time")? as i64;
    if !(-62_135_596_800..=253_402_300_799).contains(&time) {
        return Err("time");
    }
    let hostname = reader.prefixed(2).filter(|h| !h.is_empty()).ok_or("hostname")?.to_vec();
    let count = reader.uint(2).filter(|c| *c != 0).ok_or("chain")?;
    let mut chain = Vec::new();
    for _ in 0..count {
        chain.push(reader.prefixed(4).filter(|c| !c.is_empty()).ok_or("chain")?.to_vec());
    }
    let count = reader.uint(2).ok_or("trust")?;
    let mut trust = Vec::new();
    for _ in 0..count {
        let kind = match reader.uint(1).ok_or("trust kind")? {
            0 => TrustKind::Anchor,
            1 => TrustKind::AnchorWithEnforcedConstraints,
            2 => TrustKind::AnchorWithConstraints,
            3 => TrustKind::Leaf,
            4 => TrustKind::AnchorOrLeaf,
            5 => TrustKind::UntrustedAuthority,
            6 => TrustKind::DistrustedSpki,
            _ => return Err("trust kind"),
        };
        let der = reader.prefixed(4).filter(|c| !c.is_empty()).ok_or("trust certificate")?.to_vec();
        let mut entry = TrustEntry { kind, der, dns_names: Vec::new(), cidrs: Vec::new() };
        if matches!(kind, TrustKind::AnchorWithConstraints | TrustKind::Leaf | TrustKind::AnchorOrLeaf) {
            for _ in 0..reader.uint(2).ok_or("trust names")? {
                entry.dns_names.push(reader.prefixed(2).ok_or("trust names")?.to_vec());
            }
            for _ in 0..reader.uint(2).ok_or("trust cidrs")? {
                let width = reader.uint(1).filter(|w| *w == 4 || *w == 16).ok_or("trust cidrs")? as usize;
                let address = reader.bytes(width).ok_or("trust cidrs")?.to_vec();
                let mask = reader.bytes(width).ok_or("trust cidrs")?.to_vec();
                entry.cidrs.push((address, mask));
            }
        }
        trust.push(entry);
    }
    let mut scts = Vec::new();
    let mut ocsp = Vec::new();
    if !reader.data.is_empty() {
        scts = reader.prefixed(2).ok_or("sct list")?.to_vec();
        if !reader.data.is_empty() {
            ocsp = reader.prefixed(2).ok_or("ocsp response")?.to_vec();
        }
    }
    if !reader.data.is_empty() {
        return Err("trailing bytes");
    }
    Ok(Request { time, hostname, chain, trust, scts, ocsp })
}

fn error_name(error: i32) -> &'static str {
    match error {
        0 => "OK",
        -150 => "ERR_SSL_PINNED_KEY_NOT_IN_CERT_CHAIN",
        -167 => "ERR_SSL_SERVER_CERT_BAD_FORMAT",
        -200 => "ERR_CERT_COMMON_NAME_INVALID",
        -201 => "ERR_CERT_DATE_INVALID",
        -202 => "ERR_CERT_AUTHORITY_INVALID",
        -204 => "ERR_CERT_NO_REVOCATION_MECHANISM",
        -205 => "ERR_CERT_UNABLE_TO_CHECK_REVOCATION",
        -206 => "ERR_CERT_REVOKED",
        -207 => "ERR_CERT_INVALID",
        -208 => "ERR_CERT_WEAK_SIGNATURE_ALGORITHM",
        -210 => "ERR_CERT_NON_UNIQUE_NAME",
        -211 => "ERR_CERT_WEAK_KEY",
        -212 => "ERR_CERT_NAME_CONSTRAINT_VIOLATION",
        -213 => "ERR_CERT_VALIDITY_TOO_LONG",
        -214 => "ERR_CERTIFICATE_TRANSPARENCY_REQUIRED",
        -217 => "ERR_CERT_KNOWN_INTERCEPTION_BLOCKED",
        other => panic!("net error {other} is outside the verifier's results"),
    }
}

const STATUS_NAMES: [(u32, &str); 21] = [
    (1 << 0, "COMMON_NAME_INVALID"),
    (1 << 1, "DATE_INVALID"),
    (1 << 2, "AUTHORITY_INVALID"),
    (1 << 4, "NO_REVOCATION_MECHANISM"),
    (1 << 5, "UNABLE_TO_CHECK_REVOCATION"),
    (1 << 6, "REVOKED"),
    (1 << 7, "INVALID"),
    (1 << 8, "WEAK_SIGNATURE_ALGORITHM"),
    (1 << 10, "NON_UNIQUE_NAME"),
    (1 << 11, "WEAK_KEY"),
    (1 << 13, "PINNED_KEY_MISSING"),
    (1 << 14, "NAME_CONSTRAINT_VIOLATION"),
    (1 << 15, "VALIDITY_TOO_LONG"),
    (1 << 16, "IS_EV"),
    (1 << 17, "REV_CHECKING_ENABLED"),
    (1 << 19, "SHA1_SIGNATURE_PRESENT"),
    (1 << 21, "KNOWN_INTERCEPTION_DETECTED"),
    (1 << 22, "IS_QWAC"),
    (1 << 24, "CERTIFICATE_TRANSPARENCY_REQUIRED"),
    (1 << 26, "KNOWN_INTERCEPTION_BLOCKED"),
    (1 << 28, "SELF_SIGNED_LOCAL_NETWORK"),
];

fn compliance_name(compliance: CtCompliance) -> &'static str {
    match compliance {
        CtCompliance::Complies => "complies",
        CtCompliance::NotEnoughScts => "not-enough-scts",
        CtCompliance::NotDiverseScts => "not-diverse-scts",
        CtCompliance::BuildNotTimely => "build-not-timely",
        CtCompliance::DetailsNotAvailable => "details-not-available",
    }
}

fn render(outcome: &Outcome) -> String {
    let mut out = format!("error {} {}\n", outcome.error, error_name(outcome.error));
    if outcome.error == verify::ERR_SSL_SERVER_CERT_BAD_FORMAT {
        return out;
    }
    let names: Vec<&str> = STATUS_NAMES.iter().filter(|(flag, _)| outcome.status & flag != 0).map(|(_, name)| *name).collect();
    let known = STATUS_NAMES.iter().fold(0, |all, (flag, _)| all | flag);
    assert_eq!(outcome.status & !known, 0, "the verifier sets only named CertStatus flags");
    out += &format!("status {}\n", if names.is_empty() { "none".to_owned() } else { names.join("|") });
    out += &format!("ct {}\n", compliance_name(outcome.ct));
    out += "chain";
    for der in &outcome.chain {
        out += " ";
        for byte in keys::sha256(der) {
            out += &format!("{byte:02x}");
        }
    }
    out += "\n";
    out
}

fn execute(verifier: &Verifier, input: &[u8]) -> String {
    match parse_request(input) {
        Err(reason) => format!("invalid {reason}\n"),
        Ok(request) => render(&verifier.verify(request.time, &request.hostname, &request.chain, &request.trust, &request.scts, &request.ocsp)),
    }
}

fn main() {
    let verifier = Verifier::new(trust_data::load());
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let mut magic = [0u8; 4];
    stdin.read_exact(&mut magic).expect("the peer sends the protocol header");
    assert_eq!(&magic, b"MGB1", "the peer speaks protocol version 1");
    stdout.write_all(b"MGB1").and_then(|()| stdout.flush()).expect("stdout accepts the protocol header");
    loop {
        let mut header = [0u8; 4];
        match stdin.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return,
            Err(error) => panic!("reading a request length failed: {error}"),
        }
        let mut request = vec![0u8; u32::from_be_bytes(header) as usize];
        stdin.read_exact(&mut request).expect("the peer sends the announced request bytes");
        let response = execute(&verifier, &request);
        let length = u32::try_from(response.len()).expect("responses fit the frame length");
        stdout.write_all(&length.to_be_bytes()).and_then(|()| stdout.write_all(response.as_bytes())).and_then(|()| stdout.flush()).expect("stdout accepts the response");
    }
}
