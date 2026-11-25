use std::collections::BTreeMap;

#[derive(Debug)]
pub enum Declaration {
    Present(String),
    None { reason: String },
}

#[derive(Debug)]
pub enum Comparator {
    Exact { surface: String },
    Tolerance { bound: f64, surface: String },
}

#[derive(Debug)]
pub enum Arm64Evidence {
    Captures(String),
    PublicApi(String),
    Harness(String),
    Assembly(String),
}

#[derive(Debug)]
pub struct Spec {
    pub(crate) seam: String,
    pub(crate) streaming: Declaration,
    pub(crate) reentrancy: Declaration,
    pub(crate) callbacks: Declaration,
    pub(crate) threading: Declaration,
    pub(crate) wire: u32,
    pub(crate) corpus: String,
    pub(crate) environment: Declaration,
    pub(crate) comparator: Comparator,
    pub(crate) varying: Declaration,
    pub(crate) ordering: Declaration,
    pub(crate) defects: Declaration,
    pub(crate) improvements: Declaration,
    pub(crate) anchor: Declaration,
    pub(crate) arm64: Arm64Evidence,
    pub(crate) holdout_percent: u8,
    pub(crate) oracle: String,
    pub(crate) candidate: String,
}

fn nonempty(value: &str, key: &str) -> Result<String, String> {
    if value.trim().is_empty() { return Err(format!("{key} must not be empty")); }
    Ok(value.to_owned())
}

fn declaration(value: &str, key: &str) -> Result<Declaration, String> {
    if let Some(reason) = value.strip_prefix("none: ") {
        Ok(Declaration::None { reason: nonempty(reason, key)? })
    } else if value == "none" || value.starts_with("none:") {
        Err(format!("{key}: none requires a reason"))
    } else {
        Ok(Declaration::Present(nonempty(value, key)?))
    }
}

enum RawValue { String(String), Integer(String) }

impl Spec {
    pub fn seam(&self) -> &str { &self.seam }
    pub fn streaming(&self) -> &Declaration { &self.streaming }
    pub fn reentrancy(&self) -> &Declaration { &self.reentrancy }
    pub fn callbacks(&self) -> &Declaration { &self.callbacks }
    pub fn threading(&self) -> &Declaration { &self.threading }
    pub fn wire_version(&self) -> u32 { self.wire }
    pub fn corpus_sources(&self) -> &str { &self.corpus }
    pub fn environment(&self) -> &Declaration { &self.environment }
    pub fn comparator(&self) -> &Comparator { &self.comparator }
    pub fn varying_relationships(&self) -> &Declaration { &self.varying }
    pub fn ordering(&self) -> &Declaration { &self.ordering }
    pub fn oracle_defects(&self) -> &Declaration { &self.defects }
    pub fn improvements(&self) -> &Declaration { &self.improvements }
    pub fn anchor(&self) -> &Declaration { &self.anchor }
    pub fn arm64_evidence(&self) -> &Arm64Evidence { &self.arm64 }
    pub fn holdout_percent(&self) -> u8 { self.holdout_percent }
    pub fn oracle(&self) -> &str { &self.oracle }
    pub fn candidate(&self) -> &str { &self.candidate }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut values = BTreeMap::new();
        for (line, raw) in text.lines().enumerate() {
            let raw = raw.trim();
            if raw.is_empty() || raw.starts_with('#') { continue; }
            let (key, value) = raw.split_once('=').ok_or_else(|| format!("line {}: expected key = value", line + 1))?;
            let key = key.trim();
            let value = value.trim();
            if key.is_empty() || !key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
                return Err(format!("line {}: invalid key", line + 1));
            }
            let value = if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
                let s = &value[1..value.len()-1];
                if s.contains(['"', '\\']) { return Err(format!("line {}: escapes are not supported", line + 1)); }
                RawValue::String(s.to_owned())
            } else if value.bytes().all(|b| b.is_ascii_digit()) && !value.is_empty() {
                if value.len() > 1 && value.starts_with('0') { return Err(format!("line {}: integer has a leading zero", line + 1)); }
                RawValue::Integer(value.to_owned())
            } else { return Err(format!("line {}: expected a basic string or integer", line + 1)); };
            if values.insert(key.to_owned(), value).is_some() { return Err(format!("duplicate {key}")); }
        }
        fn take(values: &mut BTreeMap<String, RawValue>, key: &str) -> Result<String, String> {
            let value = values.remove(key).ok_or_else(|| format!("missing {key}"))?;
            match value { RawValue::String(value) => nonempty(&value, key), RawValue::Integer(_) => Err(format!("{key} must be a string")) }
        }
        fn integer(values: &mut BTreeMap<String, RawValue>, key: &str) -> Result<String, String> {
            match values.remove(key).ok_or_else(|| format!("missing {key}"))? {
                RawValue::Integer(value) => Ok(value), RawValue::String(_) => Err(format!("{key} must be an integer")),
            }
        }
        let seam = take(&mut values, "seam")?;
        let streaming = declaration(&take(&mut values, "streaming")?, "streaming")?;
        let reentrancy = declaration(&take(&mut values, "reentrancy")?, "reentrancy")?;
        let callbacks = declaration(&take(&mut values, "callbacks")?, "callbacks")?;
        let threading = declaration(&take(&mut values, "threading")?, "threading")?;
        let wire = integer(&mut values, "wire_version")?.parse::<u32>().map_err(|_| "invalid wire_version")?;
        if wire != 1 { return Err("unsupported wire_version".into()); }
        let corpus = take(&mut values, "corpus_sources")?;
        let environment = declaration(&take(&mut values, "environment")?, "environment")?;
        let comparison = take(&mut values, "comparison")?;
        let surface = take(&mut values, "observable_surface")?;
        let comparator = match comparison.as_str() {
            "exact" => Comparator::Exact { surface },
            "tolerance" => {
                let bound = take(&mut values, "tolerance_bound")?.parse::<f64>().map_err(|_| "invalid tolerance_bound")?;
                if !bound.is_finite() || bound < 0.0 { return Err("invalid tolerance_bound".into()); }
                Comparator::Tolerance { bound, surface }
            }
            _ => return Err("invalid comparison".into()),
        };
        let varying = declaration(&take(&mut values, "varying_relationships")?, "varying_relationships")?;
        if let Declaration::Present(s) = &varying {
            let check = s.split_once("check: ").map(|(_, value)| value.trim()).unwrap_or("");
            if check.is_empty() { return Err("varying_relationships requires a relationship check".into()); }
        }
        let ordering = declaration(&take(&mut values, "ordering")?, "ordering")?;
        let defects = declaration(&take(&mut values, "oracle_defects")?, "oracle_defects")?;
        let improvements = declaration(&take(&mut values, "improvements")?, "improvements")?;
        for (key, item) in [("oracle_defects", &defects), ("improvements", &improvements)] {
            if let Declaration::Present(s) = item {
                let assertion = s.split_once("assert: ").map(|(_, value)| value.trim()).unwrap_or("");
                if assertion.is_empty() { return Err(format!("{key} requires an absolute assertion")); }
            }
        }
        let anchor = declaration(&take(&mut values, "anchor")?, "anchor")?;
        let arm = take(&mut values, "arm64_evidence")?;
        let (kind, detail) = arm.split_once(": ").ok_or("arm64_evidence needs kind and detail")?;
        let detail = nonempty(detail, "arm64_evidence")?;
        let arm64 = match kind {
            "captures" => Arm64Evidence::Captures(detail),
            "public_api" => Arm64Evidence::PublicApi(detail),
            "harness" => Arm64Evidence::Harness(detail),
            "assembly" => Arm64Evidence::Assembly(detail),
            _ => return Err("invalid arm64_evidence".into()),
        };
        let holdout_percent = integer(&mut values, "holdout_percent")?.parse::<u8>().map_err(|_| "invalid holdout_percent")?;
        if !(30..=99).contains(&holdout_percent) { return Err("holdout_percent must be 30..99".into()); }
        let oracle = take(&mut values, "oracle")?;
        let candidate = take(&mut values, "candidate")?;
        if !values.is_empty() { return Err(format!("unknown keys: {:?}", values.keys())); }
        Ok(Self { seam, streaming, reentrancy, callbacks, threading, wire, corpus, environment, comparator, varying, ordering, defects, improvements, anchor, arm64, holdout_percent, oracle, candidate })
    }
}
