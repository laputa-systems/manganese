use serde::Deserialize;
use serde_json_lenient::{de::SliceRead, Deserializer, Value};

#[repr(C)]
pub struct JsonNumber {
    tag: u8,
    size: u8,
    data: [u8; 8],
}

#[no_mangle]
pub extern "C" fn json_number(input: &[u8; 65536], len: u32) -> JsonNumber {
    let mut result = JsonNumber { tag: 0, size: 0, data: [0; 8] };
    let Some(data) = input.get(..len as usize) else { return result; };
    let json = data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(data);
    let mut de = Deserializer::new(SliceRead::new(json, false, false, false, false, false));
    let parsed = Value::deserialize(&mut de).and_then(|v| de.end().map(|()| v));
    if let Ok(Value::Number(number)) = parsed {
        if let Some(n) = number.as_i64() {
            if let Ok(n) = i32::try_from(n) {
                result.tag = 1; result.size = 4; result.data[..4].copy_from_slice(&n.to_be_bytes());
            } else {
                result.tag = 2; result.size = 8; result.data.copy_from_slice(&(n as f64).to_bits().to_be_bytes());
            }
        } else if let Some(n) = number.as_u64() {
            if let Ok(n) = i32::try_from(n) {
                result.tag = 1; result.size = 4; result.data[..4].copy_from_slice(&n.to_be_bytes());
            } else {
                result.tag = 2; result.size = 8; result.data.copy_from_slice(&(n as f64).to_bits().to_be_bytes());
            }
        } else if let Some(n) = number.as_f64() {
            result.tag = 2; result.size = 8; result.data.copy_from_slice(&n.to_bits().to_be_bytes());
        }
    }
    result
}
