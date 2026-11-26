use crate::cert;
use crate::der;
use crate::der::BitString;
use crate::keys;
use crate::path::CtCompliance;
use crate::path::SctOrigin;
use crate::path::VerifiedSct;
use crate::signature::Digest;
use crate::signature::SignatureAlgorithm;
use crate::trust_data::CtLog;
use crate::trust_data::CtLogType;

const OID_EMBEDDED_SCT: &[u8] = &[0x2b, 0x06, 0x01, 0x04, 0x01, 0xd6, 0x79, 0x02, 0x04, 0x02];
const OID_OCSP_SCT: &[u8] = &[0x2b, 0x06, 0x01, 0x04, 0x01, 0xd6, 0x79, 0x02, 0x04, 0x05];
const OID_BASIC_RESPONSE: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01, 0x01];

struct ParsedSct<'a> {
    log_id: [u8; 32],
    timestamp_ms: u64,
    extensions: &'a [u8],
    hash_algorithm: u8,
    signature_algorithm: u8,
    signature: &'a [u8],
}

fn parse_sct(data: &[u8]) -> Option<ParsedSct<'_>> {
    let (version, rest) = data.split_first()?;
    if *version != 0 {
        return None;
    }
    let (id, rest) = rest.split_at_checked(32)?;
    let (timestamp, rest) = rest.split_at_checked(8)?;
    let (length, rest) = rest.split_at_checked(2)?;
    let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
    let (extensions, rest) = rest.split_at_checked(length)?;
    let (hash_algorithm, rest) = rest.split_first()?;
    let (signature_algorithm, rest) = rest.split_first()?;
    let (length, rest) = rest.split_at_checked(2)?;
    let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
    if length == 0 {
        return None;
    }
    let (signature, rest) = rest.split_at_checked(length)?;
    if !rest.is_empty() {
        return None;
    }
    let mut log_id = [0u8; 32];
    log_id.copy_from_slice(id);
    Some(ParsedSct {
        log_id,
        timestamp_ms: u64::from_be_bytes([timestamp[0], timestamp[1], timestamp[2], timestamp[3], timestamp[4], timestamp[5], timestamp[6], timestamp[7]]),
        extensions,
        hash_algorithm: *hash_algorithm,
        signature_algorithm: *signature_algorithm,
        signature,
    })
}

fn parse_list(data: &[u8]) -> Option<Vec<Vec<u8>>> {
    let (length, rest) = data.split_at_checked(2)?;
    let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
    let (list, rest) = rest.split_at_checked(length)?;
    if !rest.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    let mut remaining = list;
    while !remaining.is_empty() {
        let (length, tail) = remaining.split_at_checked(2)?;
        let size = usize::from(u16::from_be_bytes([length[0], length[1]]));
        if size == 0 {
            return None;
        }
        let (item, tail) = tail.split_at_checked(size)?;
        out.push(item.to_vec());
        remaining = tail;
    }
    Some(out)
}

fn embedded_list(leaf_der: &[u8]) -> Option<Vec<Vec<u8>>> {
    let extensions = cert::parse_extensions(cert::parse_tbs(cert::parse_outer(leaf_der)?.tbs_tlv)?.extensions_tlv?)?;
    let value = extensions.get(OID_EMBEDDED_SCT)?;
    let inner = der::Parser::new(&value.value).read_tag(der::OCTET_STRING)?;
    parse_list(inner)
}

fn ocsp_list(ocsp: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut response = der::Parser::new(ocsp);
    let mut body = response.read_sequence()?;
    let response_bytes = body.read_raw_tlv()?;
    if body.has_more() || response.has_more() {
        return None;
    }
    let mut basic = der::Parser::new(response_bytes);
    let mut sequence = basic.read_sequence()?;
    let oid = sequence.read_tag(der::OBJECT)?;
    if oid != OID_BASIC_RESPONSE {
        return None;
    }
    let contents = sequence.read_tag(der::OCTET_STRING)?;
    let mut basic_response = der::Parser::new(contents);
    let mut responses = basic_response.read_sequence()?;
    while responses.has_more() {
        if let Some(list) = find_ocsp_scts(responses.read_raw_tlv()?) {
            return Some(list);
        }
    }
    None
}

fn find_ocsp_scts(single_response: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut parser = der::Parser::new(single_response);
    let mut response = parser.read_sequence()?;
    response.read_raw_tlv()?;
    while response.has_more() {
        let tag = response.read_raw_tlv()?;
        if !tag.starts_with(&[0xa1]) {
            continue;
        }
        let mut outer = der::Parser::new(tag);
        let constructed = outer.read_constructed(der::context_constructed(1))?;
        let mut extensions = constructed;
        while extensions.has_more() {
            let extension = extensions.read_raw_tlv()?;
            let mut seq = der::Parser::new(extension);
            let mut fields = seq.read_sequence()?;
            let oid = fields.read_tag(der::OBJECT)?;
            if oid != OID_OCSP_SCT {
                continue;
            }
            let value = fields.read_tag(der::OCTET_STRING)?;
            let inner = der::Parser::new(value).read_tag(der::OCTET_STRING)?;
            return parse_list(inner);
        }
    }
    None
}

fn algorithm(hash: u8, signature: u8) -> Option<SignatureAlgorithm> {
    match (hash, signature) {
        (4, 1) => Some(SignatureAlgorithm::RsaPkcs1(Digest::Sha256)),
        (5, 1) => Some(SignatureAlgorithm::RsaPkcs1(Digest::Sha384)),
        (6, 1) => Some(SignatureAlgorithm::RsaPkcs1(Digest::Sha512)),
        (4, 3) => Some(SignatureAlgorithm::Ecdsa(Digest::Sha256)),
        (5, 3) => Some(SignatureAlgorithm::Ecdsa(Digest::Sha384)),
        (6, 3) => Some(SignatureAlgorithm::Ecdsa(Digest::Sha512)),
        _ => None,
    }
}

fn length_prefixed24(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() > 0x00ff_ffff {
        return None;
    }
    let length = data.len();
    let mut out = Vec::with_capacity(length + 3);
    out.push((length >> 16) as u8);
    out.push((length >> 8) as u8);
    out.push(length as u8);
    out.extend_from_slice(data);
    Some(out)
}

fn precert_tbs(leaf_der: &[u8]) -> Option<Vec<u8>> {
    let tbs_tlv = cert::parse_outer(leaf_der)?.tbs_tlv;
    let mut parser = der::Parser::new(tbs_tlv).read_sequence()?;
    let mut fields = Vec::new();
    if let Some(version) = parser.read_optional_tag(der::context_constructed(0))? {
        fields.push(der::encode(der::context_constructed(0), version));
    }
    for _ in 0..6 {
        let tlv = match parser.read_raw_tlv() {
            Some(value) => value,
            None => return None,
        };
        fields.push(tlv.to_vec());
    }
    for tag in [der::context(1), der::context(2)] {
        if let Some(value) = parser.read_optional_tag(tag)? {
            fields.push(der::encode(tag, value));
        }
    }
    let extensions_tlv = match parser.read_tag(der::context_constructed(3)) {
        Some(value) => value,
        None => return None,
    };
    if parser.has_more() {
        return None;
    }
    let mut extensions = match der::Parser::new(extensions_tlv).read_sequence() {
        Some(parser) => parser,
        None => return None,
    };
    let mut kept = Vec::new();
    while extensions.has_more() {
        let extension = extensions.read_raw_tlv()?;
        let mut parser = der::Parser::new(extension);
        let mut sequence = parser.read_sequence()?;
        let oid = sequence.read_tag(der::OBJECT)?;
        if oid != OID_EMBEDDED_SCT {
            kept.push(extension.to_vec());
        }
    }
    fields.push(der::encode(der::context_constructed(3), &der::encode(der::SEQUENCE, &kept.concat())));
    Some(der::encode(der::SEQUENCE, &fields.concat()))
}

fn has_valid_leaf_index(extensions: &[u8]) -> bool {
    let mut parser = extensions;
    let mut found = false;
    while !parser.is_empty() {
        let (kind, rest) = parser.split_first().unwrap();
        let (length, rest) = match rest.split_at_checked(2) {
            Some(value) => value,
            None => return false,
        };
        let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
        let (data, rest) = match rest.split_at_checked(length) {
            Some(value) => value,
            None => return false,
        };
        if *kind == 0 {
            if found || data.len() != 5 {
                return false;
            }
            found = true;
        }
        parser = rest;
    }
    found
}

pub(crate) fn disqualified_at(log: &CtLog, time: i64) -> bool {
    log.disqualified.is_some_and(|when| time * 1_000_000 >= when)
}

fn operator<'a>(log: &'a CtLog, timestamp_ms: u64) -> &'a [u8] {
    let moment = (timestamp_ms / 1000) as i64;
    for (name, end) in &log.previous {
        if moment < *end {
            return name;
        }
    }
    &log.operator
}

pub(crate) fn verify_scts(logs: &[CtLog], leaf_der: &[u8], issuer_der: Option<&[u8]>, tls: &[u8], ocsp: &[u8]) -> Vec<VerifiedSct> {
    let precert = precert_tbs(leaf_der).and_then(|tbs| {
        let issuer = issuer_der?;
        let spki = cert::parse_tbs(cert::parse_outer(issuer)?.tbs_tlv)?.spki_tlv;
        let hash = keys::sha256(spki);
        let mut entry = vec![0, 1];
        entry.extend_from_slice(&hash);
        entry.extend_from_slice(&length_prefixed24(&tbs)?);
        Some(entry)
    });
    let x509_entry = length_prefixed24(leaf_der).map(|mut entry| {
        let mut result = vec![0, 0];
        result.append(&mut entry);
        result
    });
    let mut inputs: Vec<(Vec<u8>, SctOrigin, Vec<u8>)> = Vec::new();
    if let Some(list) = precert {
        for sct in embedded_list(leaf_der).unwrap_or_default() {
            inputs.push((sct, SctOrigin::Embedded, list.clone()));
        }
    }
    if let Some(entry) = &x509_entry {
        if !ocsp.is_empty() {
            for sct in ocsp_list(ocsp).unwrap_or_default() {
                inputs.push((sct, SctOrigin::Ocsp, entry.clone()));
            }
        }
        if !tls.is_empty() {
            for sct in parse_list(tls).unwrap_or_default() {
                inputs.push((sct, SctOrigin::Tls, entry.clone()));
            }
        }
    }
    let mut out = Vec::new();
    for (bytes, origin, entry) in inputs {
        let Some(sct) = parse_sct(&bytes) else { continue };
        let Some(index) = logs.iter().position(|log| log.id == sct.log_id) else { continue };
        let log = &logs[index];
        let Some(algorithm) = algorithm(sct.hash_algorithm, sct.signature_algorithm) else { continue };
        if log.log_type == CtLogType::Static && !has_valid_leaf_index(sct.extensions) {
            continue;
        }
        let mut signed = vec![0, 0];
        signed.extend_from_slice(&sct.timestamp_ms.to_be_bytes());
        signed.extend_from_slice(&entry);
        signed.extend_from_slice(&(sct.extensions.len() as u16).to_be_bytes());
        signed.extend_from_slice(sct.extensions);
        let Some(key) = keys::parse_public_key(&log.key) else { continue };
        if !keys::verify_signed_data(algorithm, &signed, BitString { bytes: sct.signature, unused_bits: 0 }, &key, &log.key) {
            continue;
        }
        out.push(VerifiedSct { log_index: index, timestamp_ms: sct.timestamp_ms, origin });
    }
    out
}

fn is_timely(time: i64, log_list_micros: i64) -> bool {
    (time * 1_000_000 - log_list_micros) / (86_400 * 1_000_000) < 70
}

pub(crate) fn compliance(logs: &[CtLog], scts: &[VerifiedSct], time: i64, valid_start: i64, valid_end: i64, log_list_micros: i64) -> CtCompliance {
    if !is_timely(time, log_list_micros) {
        return CtCompliance::BuildNotTimely;
    }
    let issuance = scts
        .iter()
        .filter(|sct| !disqualified_at(&logs[sct.log_index], time))
        .map(|sct| sct.timestamp_ms / 1000)
        .min();
    let mut has_embedded = false;
    let mut has_nonembedded = false;
    let mut diverse = false;
    let mut first_operator: Option<&[u8]> = None;
    let mut embedded_ids = Vec::new();
    for sct in scts {
        let log = &logs[sct.log_index];
        let disqualified = disqualified_at(log, time);
        if disqualified && sct.origin != SctOrigin::Embedded {
            continue;
        }
        if sct.origin != SctOrigin::Embedded {
            has_nonembedded = true;
        } else {
            has_embedded |= !disqualified;
            let disqualification = log.disqualified.unwrap_or(i64::MAX).try_into().unwrap_or(u64::MAX);
            if !disqualified || (issuance.is_some_and(|when| when * 1000 < disqualification) && (sct.timestamp_ms / 1000) * 1000 < disqualification) {
                embedded_ids.push(log.id);
            }
        }
        let name = operator(log, sct.timestamp_ms);
        match first_operator {
            None => first_operator = Some(name),
            Some(first) if first != name => diverse = true,
            Some(_) => {}
        }
    }
    if has_nonembedded && diverse {
        return CtCompliance::Complies;
    }
    if !has_embedded {
        return if has_nonembedded { CtCompliance::NotDiverseScts } else { CtCompliance::NotEnoughScts };
    }
    if !diverse {
        return CtCompliance::NotDiverseScts;
    }
    embedded_ids.sort();
    embedded_ids.dedup();
    let required = if (valid_end - valid_start) > 180 * 86_400 { 3 } else { 2 };
    if embedded_ids.len() >= required {
        CtCompliance::Complies
    } else {
        CtCompliance::NotEnoughScts
    }
}
