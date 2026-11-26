pub const MATCH_CASE: u32 = 1;
pub const IGNORE_SUFFIX: u32 = 2;
pub const DONT_OVERWRITE: u32 = 16;

fn c_string(bytes: &[u8]) -> &[u8] {
    match bytes.iter().position(|&byte| byte == 0) {
        Some(end) => &bytes[..end],
        None => bytes,
    }
}

#[derive(Clone, Debug, Default)]
pub struct Dict {
    entries: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Dict {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn key(&self, index: usize) -> &[u8] {
        &self.entries[index].0
    }

    pub fn value(&self, index: usize) -> &[u8] {
        &self.entries[index].1
    }

    pub fn get(&self, key: &[u8], after: Option<usize>, flags: u32) -> Option<usize> {
        let key = c_string(key);
        let start = after.map_or(0, |index| index + 1);
        (start..self.entries.len()).find(|&index| {
            let candidate = &self.entries[index].0;
            if candidate.len() < key.len() {
                return false;
            }
            let prefix = &candidate[..key.len()];
            let same = if flags & MATCH_CASE != 0 { prefix == key } else { prefix.eq_ignore_ascii_case(key) };
            same && (candidate.len() == key.len() || flags & IGNORE_SUFFIX != 0)
        })
    }

    pub fn set(&mut self, key: &[u8], value: Option<&[u8]>, flags: u32) {
        let key = c_string(key).to_vec();
        let value = value.map(|value| c_string(value).to_vec());
        if let Some(index) = self.get(&key, None, flags) {
            if flags & DONT_OVERWRITE != 0 {
                return;
            }
            self.entries.swap_remove(index);
        }
        if let Some(value) = value {
            self.entries.push((key, value));
        }
    }

    pub fn convert(&mut self, table: &[(&str, &str)]) {
        let mut converted = Dict::default();
        for (key, value) in &self.entries {
            let key = table
                .iter()
                .find(|(native, _)| native.as_bytes().eq_ignore_ascii_case(key))
                .map_or(&key[..], |(_, generic)| generic.as_bytes());
            converted.set(key, Some(value), 0);
        }
        *self = converted;
    }
}
