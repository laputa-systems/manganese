use numerics::{write_json_double, JsonDoubleStyle};
use quick_xml::Reader;
use quick_xml::events::Event;
use serde::de::{DeserializeSeed, Deserializer as SerdeDeserializer, Error as DeError, MapAccess, SeqAccess, Visitor};
use serde_json_lenient::de::SliceRead;
use serde_json_lenient::Deserializer;
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};

const UNKNOWN_POSITION: i32 = -1;
const MAX_RECORD: usize = 16 * 1024 * 1024;

fn fail(message: &str) -> ! {
    let _ = writeln!(io::stderr(), "structured-data: {message}");
    std::process::exit(1);
}

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Null,
    Bool(bool),
    Int(i32),
    Double(f64),
    String(String),
    List(Vec<Value>),
    Dict(BTreeMap<String, Value>),
}

struct ReaderBytes<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> ReaderBytes<'a> {
    fn byte(&mut self) -> Option<u8> {
        let value = *self.data.get(self.offset)?;
        self.offset += 1;
        Some(value)
    }
    fn word(&mut self) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..4 {
            value = value.checked_mul(256)?.checked_add(u32::from(self.byte()?))?;
        }
        Some(value)
    }
    fn blob(&mut self) -> Option<&'a [u8]> {
        let length = self.word()? as usize;
        let end = self.offset.checked_add(length)?;
        let value = self.data.get(self.offset..end)?;
        self.offset = end;
        Some(value)
    }
    fn string(&mut self) -> Option<String> {
        std::str::from_utf8(self.blob()?).ok().map(str::to_owned)
    }
}

fn number(out: &mut Vec<u8>, value: u32) {
    out.extend(value.to_be_bytes());
}

fn blob(out: &mut Vec<u8>, value: &[u8]) {
    number(out, value.len() as u32);
    out.extend_from_slice(value);
}

fn dump_value(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.push(0),
        Value::Bool(false) => out.push(1),
        Value::Bool(true) => out.push(2),
        Value::Int(value) => {
            out.push(3);
            out.extend(value.to_be_bytes());
        }
        Value::Double(value) => {
            out.push(4);
            out.extend(value.to_bits().to_be_bytes());
        }
        Value::String(value) => {
            out.push(5);
            blob(out, value.as_bytes());
        }
        Value::List(items) => {
            out.push(6);
            number(out, items.len() as u32);
            for item in items {
                dump_value(item, out);
            }
        }
        Value::Dict(items) => {
            out.push(7);
            number(out, items.len() as u32);
            for (key, item) in items {
                blob(out, key.as_bytes());
                dump_value(item, out);
            }
        }
    }
}

fn read_value(reader: &mut ReaderBytes<'_>, depth: usize) -> Option<Value> {
    if depth > 200 {
        return None;
    }
    match reader.byte()? {
        0 => Some(Value::Null),
        1 => Some(Value::Bool(false)),
        2 => Some(Value::Bool(true)),
        3 => {
            let bits = reader.word()?;
            Some(Value::Int(i32::from_be_bytes(bits.to_be_bytes())))
        }
        4 => {
            let mut bits = 0u64;
            for _ in 0..8 {
                bits = bits.checked_mul(256)?.checked_add(u64::from(reader.byte()?))?;
            }
            Some(Value::Double(f64::from_bits(bits)))
        }
        5 => Some(Value::String(reader.string()?)),
        6 => {
            let count = reader.word()? as usize;
            if count > 100_000 {
                return None;
            }
            let mut items = Vec::with_capacity(count.min(1024));
            for _ in 0..count {
                items.push(read_value(reader, depth + 1)?);
            }
            Some(Value::List(items))
        }
        7 => {
            let count = reader.word()? as usize;
            if count > 100_000 {
                return None;
            }
            let mut items = BTreeMap::new();
            for _ in 0..count {
                let key = reader.string()?;
                let value = read_value(reader, depth + 1)?;
                items.insert(key, value);
            }
            Some(Value::Dict(items))
        }
        _ => None,
    }
}

struct ChromiumVisitor {
    depth: usize,
}

impl ChromiumVisitor {
    fn recurse<E: DeError>(&self) -> Result<usize, E> {
        self.depth
            .checked_sub(1)
            .ok_or_else(|| E::custom("recursion limit exceeded"))
    }
}

impl<'de> DeserializeSeed<'de> for ChromiumVisitor {
    type Value = Value;

    fn deserialize<D: SerdeDeserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for ChromiumVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any valid JSON")
    }

    fn visit_i8<E: DeError>(self, value: i8) -> Result<Self::Value, E> {
        self.visit_i32(i32::from(value))
    }
    fn visit_i32<E: DeError>(self, value: i32) -> Result<Self::Value, E> {
        Ok(Value::Int(value))
    }
    fn visit_i64<E: DeError>(self, value: i64) -> Result<Self::Value, E> {
        Ok(i32::try_from(value).map_or_else(|_| Value::Double(value as f64), Value::Int))
    }
    fn visit_u64<E: DeError>(self, value: u64) -> Result<Self::Value, E> {
        Ok(i32::try_from(value).map_or_else(|_| Value::Double(value as f64), Value::Int))
    }
    fn visit_f32<E: DeError>(self, value: f32) -> Result<Self::Value, E> {
        self.visit_f64(f64::from(value))
    }
    fn visit_f64<E: DeError>(self, value: f64) -> Result<Self::Value, E> {
        Ok(Value::Double(value))
    }
    fn visit_bool<E: DeError>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_str<E: DeError>(self, value: &str) -> Result<Self::Value, E> {
        Ok(Value::String(value.to_owned()))
    }
    fn visit_borrowed_str<E: DeError>(self, value: &'de str) -> Result<Self::Value, E> {
        self.visit_str(value)
    }
    fn visit_string<E: DeError>(self, value: String) -> Result<Self::Value, E> {
        Ok(Value::String(value))
    }
    fn visit_char<E: DeError>(self, value: char) -> Result<Self::Value, E> {
        self.visit_string(value.to_string())
    }
    fn visit_none<E: DeError>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }
    fn visit_unit<E: DeError>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }
    fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let depth = self.recurse()?;
        let mut items = BTreeMap::new();
        while let Some(key) = access.next_key::<String>()? {
            let value = access.next_value_seed(ChromiumVisitor { depth })?;
            items.insert(key, value);
        }
        Ok(Value::Dict(items))
    }
    fn visit_seq<S>(self, mut access: S) -> Result<Self::Value, S::Error>
    where
        S: SeqAccess<'de>,
    {
        let depth = self.recurse()?;
        let mut items = Vec::new();
        while let Some(value) = access.next_element_seed(ChromiumVisitor { depth })? {
            items.push(value);
        }
        Ok(Value::List(items))
    }
}

fn json_parse(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 5 {
        return None;
    }
    let options = data[0];
    let length = u32::from_be_bytes(data[1..5].try_into().ok()?) as usize;
    if length != data.len() - 5 || options > 63 {
        return None;
    }
    let input = &data[5..];
    let input = input.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(input);
    let read = SliceRead::new(
        input,
        options & 2 != 0,
        options & 32 != 0,
        false,
        options & 8 != 0,
        options & 16 != 0,
    );
    let mut de = Deserializer::new(read);
    de.set_ignore_trailing_commas(options & 1 != 0);
    de.set_allow_comments(options & 4 != 0);
    de.disable_recursion_limit();
    let parsed = ChromiumVisitor { depth: 199 }
        .deserialize(&mut de)
        .and_then(|value| de.end().map(|()| value));
    let mut out = Vec::new();
    match parsed {
        Ok(value) => {
            out.push(1);
            dump_value(&value, &mut out);
        }
        Err(error) => {
            out.push(0);
            out.extend(i32::try_from(error.line()).unwrap_or(UNKNOWN_POSITION).to_be_bytes());
            out.extend(i32::try_from(error.column()).unwrap_or(UNKNOWN_POSITION).to_be_bytes());
            blob(&mut out, error.to_string().as_bytes());
        }
    }
    Some(out)
}

fn escape_json_string(value: &str, out: &mut String) {
    out.push('"');
    for character in value.chars() {
        match character {
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '<' => out.push_str("\\u003C"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            value if (value as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04X}", value as u32));
            }
            value => out.push(value),
        }
    }
    out.push('"');
}

fn write_json(value: &Value, options: u8, depth: usize, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(value) => out.push_str(&value.to_string()),
        Value::Double(value) => {
            let style = if options & 1 != 0 {
                JsonDoubleStyle::OmitType
            } else {
                JsonDoubleStyle::PreserveType
            };
            out.push_str(&write_json_double(*value, style));
        }
        Value::String(value) => escape_json_string(value, out),
        Value::List(items) => {
            out.push('[');
            let pretty = options & 2 != 0;
            if pretty {
                out.push(' ');
            }
            for (index, item) in items.iter().enumerate() {
                if index != 0 {
                    out.push(',');
                    if pretty {
                        out.push(' ');
                    }
                }
                write_json(item, options, depth, out);
            }
            if pretty {
                out.push(' ');
            }
            out.push(']');
        }
        Value::Dict(items) => {
            out.push('{');
            let pretty = options & 2 != 0;
            if pretty {
                out.push('\n');
            }
            for (index, (key, item)) in items.iter().enumerate() {
                if index != 0 {
                    out.push(',');
                    if pretty {
                        out.push('\n');
                    }
                }
                if pretty {
                    out.push_str(&"   ".repeat(depth + 1));
                }
                escape_json_string(key, out);
                out.push(':');
                if pretty {
                    out.push(' ');
                }
                write_json(item, options, depth + 1, out);
            }
            if pretty {
                if !items.is_empty() {
                    out.push('\n');
                }
                out.push_str(&"   ".repeat(depth));
            }
            out.push('}');
        }
    }
}

fn json_write(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 2 || data[0] > 3 {
        return None;
    }
    let mut reader = ReaderBytes {
        data: &data[1..],
        offset: 0,
    };
    let value = read_value(&mut reader, 0)?;
    if reader.offset != reader.data.len() {
        return None;
    }
    let mut text = String::new();
    write_json(&value, data[0], 0, &mut text);
    if data[0] & 2 != 0 {
        text.push('\n');
    }
    let mut out = vec![1];
    blob(&mut out, text.as_bytes());
    Some(out)
}

#[derive(Clone, Debug)]
enum Node {
    Element {
        name: String,
        attributes: Vec<(String, String)>,
        namespaces: Vec<(String, String)>,
        children: Vec<Node>,
    },
    Text(String),
    CData(String),
    Comment(String),
    Pi(String, String),
}

struct Frame {
    name: String,
    attributes: Vec<(String, String)>,
    namespaces: Vec<(String, String)>,
    children: Vec<Node>,
}

fn element_parts(e: &quick_xml::events::BytesStart<'_>) -> Option<(String, Vec<(String, String)>, Vec<(String, String)>)> {
    let name = e.name().as_ref().to_owned();
    let mut attributes = Vec::new();
    let mut namespaces = Vec::new();
    for attribute in e.attributes() {
        let attribute = attribute.ok()?;
        let key = attribute.key.as_ref().to_owned();
        let value = attribute.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok()?.into_owned();
        if key == "xmlns" {
            namespaces.push((String::new(), value));
        } else if let Some(prefix) = key.strip_prefix("xmlns:") {
            namespaces.push((prefix.to_owned(), value));
        } else {
            attributes.push((key, value));
        }
    }
    Some((name, attributes, namespaces))
}

fn coalesce_text(nodes: &mut Vec<Node>) {
    let mut result = Vec::with_capacity(nodes.len());
    for node in nodes.drain(..) {
        match (result.last_mut(), node) {
            (Some(Node::Text(previous)), Node::Text(next)) => previous.push_str(&next),
            (_, mut node) => {
                if let Node::Element { children, .. } = &mut node {
                    coalesce_text(children);
                }
                result.push(node);
            }
        }
    }
    *nodes = result;
}

fn xml_parse(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 8 {
        return None;
    }
    let options = u32::from_be_bytes(data[0..4].try_into().ok()?);
    let length = u32::from_be_bytes(data[4..8].try_into().ok()?) as usize;
    if length != data.len() - 8 || options != 0 {
        return None;
    }
    let mut reader = Reader::from_reader(&data[8..]);
    reader.config_mut().check_end_names = true;
    reader.config_mut().check_comments = true;
    reader.config_mut().expand_empty_elements = false;
    let mut stack: Vec<Frame> = Vec::new();
    let mut roots: Vec<Node> = Vec::new();
    let mut encoding = String::new();
    loop {
        let event = reader.read_event().ok()?;
        match event {
            Event::Start(e) => {
                let (name, attributes, namespaces) = element_parts(&e)?;
                stack.push(Frame { name, attributes, namespaces, children: Vec::new() });
            }
            Event::Empty(e) => {
                let (name, attributes, namespaces) = element_parts(&e)?;
                let node = Node::Element { name, attributes, namespaces, children: Vec::new() };
                if let Some(frame) = stack.last_mut() {
                    frame.children.push(node);
                } else if roots.iter().all(|root| !matches!(root, Node::Element { .. })) {
                    roots.push(node);
                } else {
                    return None;
                }
            }
            Event::End(e) => {
                let frame = stack.pop()?;
                if e.name().as_ref() != frame.name.as_str() {
                    return None;
                }
                let node = Node::Element {
                    name: frame.name,
                    attributes: frame.attributes,
                    namespaces: frame.namespaces,
                    children: frame.children,
                };
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else if roots.iter().all(|root| !matches!(root, Node::Element { .. })) {
                    roots.push(node);
                } else {
                    return None;
                }
            }
            Event::Text(e) => {
                let text = quick_xml::escape::unescape(e.as_ref()).ok()?.into_owned();
                if let Some(frame) = stack.last_mut() {
                    frame.children.push(Node::Text(text));
                } else if !text.trim().is_empty() {
                    return None;
                }
            }
            Event::CData(e) => {
                let text = e.as_ref().to_owned();
                if let Some(frame) = stack.last_mut() {
                    frame.children.push(Node::CData(text));
                } else {
                    return None;
                }
            }
            Event::Comment(e) => {
                let text = e.as_ref().to_owned();
                if let Some(frame) = stack.last_mut() {
                    frame.children.push(Node::Comment(text));
                } else {
                    roots.push(Node::Comment(text));
                }
            }
            Event::PI(e) => {
                let text = e.as_ref().to_owned();
                let text = text.trim_start_matches("<?").trim_end_matches("?>");
                let (target, data) = match text.split_once(char::is_whitespace) {
                    Some((target, data)) => (target.to_owned(), data.trim().to_owned()),
                    None => (text.to_owned(), String::new()),
                };
                let node = Node::Pi(target, data);
                if let Some(frame) = stack.last_mut() {
                    frame.children.push(node);
                } else {
                    roots.push(node);
                }
            }
            Event::GeneralRef(e) => {
                let text = match e.resolve_char_ref().ok()? {
                    Some(character) => character.to_string(),
                    None => match e.as_ref() {
                        "amp" => "&".to_owned(),
                        "lt" => "<".to_owned(),
                        "gt" => ">".to_owned(),
                        "quot" => "\"".to_owned(),
                        "apos" => "'".to_owned(),
                        _ => return None,
                    },
                };
                match stack.last_mut() {
                    Some(frame) => frame.children.push(Node::Text(text)),
                    None => return None,
                }
            }
            Event::Decl(e) => {
                if !stack.is_empty() || !roots.is_empty() {
                    return None;
                }
                let text = e.as_ref();
                if let Some(start) = text.find("encoding=") {
                    let rest = &text[start + 9..];
                    let quote = rest.chars().next()?;
                    let end = rest[1..].find(quote)? + 1;
                    encoding = rest[1..end].to_owned();
                }
            }
            Event::DocType(_) => return None,
            Event::Eof => break,
        }
    }
    if !stack.is_empty() || roots.iter().filter(|node| matches!(node, Node::Element { .. })).count() != 1 {
        return None;
    }
    coalesce_text(&mut roots);
    let mut events = Vec::new();
    let mut count = 0u32;
    for root in &roots {
        if matches!(root, Node::Element { .. }) {
            emit_node(root, &mut events, &mut count);
        }
    }
    let mut serialized = String::from("<?xml version=\"1.0\"");
    if !encoding.is_empty() {
        serialized.push_str(" encoding=\"");
        serialized.push_str(&encoding);
        serialized.push('"');
    }
    serialized.push_str("?>\n");
    for root in &roots {
        serialize_node(root, &mut serialized);
        serialized.push('\n');
    }
    let mut out = vec![1];
    number(&mut out, count);
    out.extend_from_slice(&events);
    blob(&mut out, serialized.as_bytes());
    Some(out)
}

fn emit_node(node: &Node, out: &mut Vec<u8>, count: &mut u32) {
    match node {
        Node::Element { name, attributes, namespaces, children } => {
            *count += 1;
            out.push(1);
            blob(out, name.as_bytes());
            number(out, attributes.len() as u32);
            for (key, value) in attributes {
                blob(out, key.as_bytes());
                blob(out, value.as_bytes());
            }
            number(out, namespaces.len() as u32);
            for (prefix, uri) in namespaces {
                blob(out, prefix.as_bytes());
                blob(out, uri.as_bytes());
            }
            for child in children {
                emit_node(child, out, count);
            }
            *count += 1;
            out.push(2);
            blob(out, name.as_bytes());
        }
        Node::Text(text) => {
            *count += 1;
            out.push(3);
            blob(out, text.as_bytes());
        }
        Node::CData(text) => {
            *count += 1;
            out.push(6);
            blob(out, text.as_bytes());
        }
        Node::Comment(text) => {
            *count += 1;
            out.push(4);
            blob(out, text.as_bytes());
        }
        Node::Pi(target, data) => {
            *count += 1;
            out.push(5);
            blob(out, target.as_bytes());
            blob(out, data.as_bytes());
        }
    }
}

fn escape_attribute(value: &str, out: &mut String) {
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            value if value as u32 > 0x7f => out.push_str(&format!("&#x{:X};", value as u32)),
            value => out.push(value),
        }
    }
}

fn escape_text(value: &str, out: &mut String) {
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            value if value as u32 > 0x7f => out.push_str(&format!("&#x{:X};", value as u32)),
            value => out.push(value),
        }
    }
}

fn serialize_node(node: &Node, out: &mut String) {
    match node {
        Node::Element { name, attributes, namespaces, children } => {
            if children.is_empty() {
                out.push('<');
                out.push_str(name);
                for (prefix, uri) in namespaces {
                    out.push(' ');
                    if prefix.is_empty() {
                        out.push_str("xmlns=\"");
                    } else {
                        out.push_str("xmlns:");
                        out.push_str(prefix);
                        out.push_str("=\"");
                    }
                    escape_attribute(uri, out);
                    out.push('"');
                }
                for (key, value) in attributes {
                    out.push(' ');
                    out.push_str(key);
                    out.push_str("=\"");
                    escape_attribute(value, out);
                    out.push('"');
                }
                out.push_str("/>");
                return;
            }
            out.push('<');
            out.push_str(name);
            for (prefix, uri) in namespaces {
                out.push(' ');
                if prefix.is_empty() {
                    out.push_str("xmlns=\"");
                } else {
                    out.push_str("xmlns:");
                    out.push_str(prefix);
                    out.push_str("=\"");
                }
                escape_attribute(uri, out);
                out.push('"');
            }
            for (key, value) in attributes {
                out.push(' ');
                out.push_str(key);
                out.push_str("=\"");
                escape_attribute(value, out);
                out.push('"');
            }
            out.push('>');
            for child in children {
                serialize_node(child, out);
            }
            out.push_str("</");
            out.push_str(name);
            out.push('>');
        }
        Node::Text(text) => escape_text(text, out),
        Node::CData(text) => {
            out.push_str("<![CDATA[");
            out.push_str(text);
            out.push_str("]]>");
        }
        Node::Comment(text) => {
            out.push_str("<!--");
            out.push_str(text);
            out.push_str("-->");
        }
        Node::Pi(target, data) => {
            out.push_str("<?");
            out.push_str(target);
            if !data.is_empty() {
                out.push(' ');
                out.push_str(data);
            }
            out.push_str("?>");
        }
    }
}

fn record(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return vec![0];
    }
    let result = match data[0] {
        1 => json_parse(&data[1..]),
        2 => json_write(&data[1..]),
        3 => xml_parse(&data[1..]),
        _ => None,
    };
    match result {
        Some(value) => value,
        None => vec![0],
    }
}

fn exact_read(input: &mut impl Read, length: usize) -> Option<Vec<u8>> {
    let mut data = vec![0u8; length];
    input.read_exact(&mut data).ok()?;
    Some(data)
}

fn main() {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let header = match exact_read(&mut input, 4) {
        Some(value) => value,
        None => fail("protocol header truncated"),
    };
    if header != b"MGB1" {
        fail("not a box protocol stream");
    }
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if out.write_all(b"MGB1").is_err() || out.flush().is_err() {
        fail("cannot write the protocol header");
    }
    loop {
        let prefix = match exact_read(&mut input, 4) {
            Some(value) => value,
            None => return,
        };
        let length = u32::from_be_bytes(prefix.try_into().expect("a four-byte record prefix")) as usize;
        if length > MAX_RECORD {
            fail("record exceeds the input cap");
        }
        let data = match exact_read(&mut input, length) {
            Some(value) => value,
            None => fail("record truncated"),
        };
        let response = record(&data);
        if out.write_all(&(response.len() as u32).to_be_bytes()).is_err()
            || out.write_all(&response).is_err()
            || out.flush().is_err()
        {
            fail("cannot write the response");
        }
    }
}
