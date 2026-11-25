const TOKEN: usize = 32;

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_identifier_at(raw: &[u8], start: usize) -> bool {
    start + TOKEN <= raw.len()
        && raw[start..start + TOKEN].iter().all(|byte| matches!(byte, b'0'..=b'9' | b'A'..=b'F'))
        && (start == 0 || !is_word(raw[start - 1]))
        && (start + TOKEN == raw.len() || !is_word(raw[start + TOKEN]))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

/// Renames the identifiers the endpoint generated to the fixed ones the oracle harness uses: each new 32-digit
/// uppercase hexadecimal identifier becomes `F000...01`, `F000...02` and so on in order of first appearance.
/// Identifiers that are fixed already or that the client itself sent are left alone.
pub fn rename_generated_ids(raw: &[u8], sent: &[u8], fixed: &[&str]) -> Vec<u8> {
    let mut renamed: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut out = Vec::with_capacity(raw.len());
    let mut position = 0;
    while position < raw.len() {
        if !is_identifier_at(raw, position) {
            out.push(raw[position]);
            position += 1;
            continue;
        }
        let token = &raw[position..position + TOKEN];
        position += TOKEN;
        if fixed.iter().any(|id| id.as_bytes() == token) || contains(sent, token) {
            out.extend_from_slice(token);
            continue;
        }
        let replacement = match renamed.iter().find(|(original, _)| original == token) {
            Some((_, replacement)) => replacement.clone(),
            None => {
                let replacement = format!("F{:031X}", renamed.len() + 1).into_bytes();
                renamed.push((token.to_vec(), replacement.clone()));
                replacement
            }
        };
        out.extend_from_slice(&replacement);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::rename_generated_ids;

    #[test]
    fn generated_identifiers_are_numbered_by_first_appearance() {
        let a = "A".repeat(32);
        let b = "B".repeat(32);
        let fixed = "0".repeat(31) + "1";
        let raw = format!("{a} {b} {a} {fixed} x{b}y {}", "C".repeat(32));
        let sent = "C".repeat(32);
        let renamed = String::from_utf8(rename_generated_ids(raw.as_bytes(), sent.as_bytes(), &[&fixed])).expect("text");
        assert_eq!(renamed, format!("F{:031} F{:031} F{:031} {fixed} x{b}y {sent}", 1, 2, 1));
    }
}
