use std::collections::BTreeMap;
use std::fmt::Write;

pub enum Json {
    Bool(bool),
    Str(String),
    List(Vec<Json>),
    Dict(BTreeMap<String, Json>),
}

impl Json {
    pub fn str(value: impl Into<String>) -> Self {
        Json::Str(value.into())
    }

    pub fn dict<const N: usize>(entries: [(&str, Json); N]) -> Self {
        Json::Dict(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
    }
}

const INDENT: &str = "   ";

fn escape(value: &str, out: &mut String) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '<' => out.push_str("\\u003C"),
            '\u{2028}' | '\u{2029}' => {
                let _ = write!(out, "\\u{:04X}", ch as u32);
            }
            ch if (ch as u32) < 32 || ch as u32 == 127 => {
                let _ = write!(out, "\\u{:04X}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

fn build(value: &Json, depth: usize, out: &mut String) {
    match value {
        Json::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Json::Str(value) => escape(value, out),
        Json::List(items) => {
            out.push_str("[ ");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                build(item, depth, out);
            }
            out.push_str(" ]");
        }
        Json::Dict(entries) => {
            out.push_str("{\n");
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                for _ in 0..=depth {
                    out.push_str(INDENT);
                }
                escape(key, out);
                out.push_str(": ");
                build(item, depth + 1, out);
            }
            out.push('\n');
            for _ in 0..depth {
                out.push_str(INDENT);
            }
            out.push('}');
        }
    }
}

pub fn pretty(value: &Json) -> String {
    let mut out = String::new();
    build(value, 0, &mut out);
    out.push('\n');
    out
}

pub fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            ch => out.push(ch),
        }
    }
    out
}

pub fn protocol_string(value: &str, out: &mut String) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ch if (32..=127).contains(&(ch as u32)) => out.push(ch),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ch => {
                let mut units = [0u16; 2];
                for unit in ch.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dicts_indent_three_spaces_and_lists_hug_their_elements() {
        let value = Json::List(vec![Json::dict([("b", Json::str("x<")), ("a", Json::Bool(true))]), Json::dict([("a", Json::Bool(false))])]);
        assert_eq!(pretty(&value), "[ {\n   \"a\": true,\n   \"b\": \"x\\u003C\"\n}, {\n   \"a\": false\n} ]\n");
    }

    #[test]
    fn protocol_strings_escape_like_crdtp() {
        let mut out = String::new();
        protocol_string("a\"\u{1}\u{e9}\u{1f600}", &mut out);
        assert_eq!(out, "\"a\\\"\\u0001\\u00e9\\ud83d\\ude00\"");
    }
}
