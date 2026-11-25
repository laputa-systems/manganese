use crate::cbor::Token;
use crate::cbor::Tokenizer;
use crate::status::Error;
use crate::status::Status;

/// The envelope of a DevTools command (`id`, `method`, `params`, `sessionId`), parsed the way crdtp does.
#[derive(Debug)]
pub struct Dispatchable<'a> {
    call_id: Option<i32>,
    method: &'a [u8],
    session_id: &'a [u8],
    params: Option<&'a [u8]>,
}

impl<'a> Dispatchable<'a> {
    pub fn call_id(&self) -> Option<i32> {
        self.call_id
    }

    pub fn method(&self) -> &'a [u8] {
        self.method
    }

    pub fn session_id(&self) -> &'a [u8] {
        self.session_id
    }

    pub fn params(&self) -> Option<&'a [u8]> {
        self.params
    }

    /// On failure, the call id seen before the error, if any, with the error.
    pub fn parse(cbor: &'a [u8]) -> Result<Self, (Option<i32>, Status)> {
        let mut parsed = Dispatchable { call_id: None, method: &[], session_id: &[], params: None };
        let mut params_seen = false;
        let fail = |parsed: &Dispatchable, error: Error, position: usize| Err((parsed.call_id, Status { error, position }));
        if cbor.first() != Some(&0xd8) {
            return fail(&parsed, Error::MessageMustBeAnObject, 0);
        }
        let mut tokenizer = Tokenizer::new(cbor);
        tokenizer.enter_envelope();
        if tokenizer.token() != Token::MapStart {
            return fail(&parsed, Error::MessageMustBeAnObject, tokenizer.position());
        }
        tokenizer.next();
        while tokenizer.token() != Token::Stop {
            if tokenizer.token() != Token::String8 {
                return fail(&parsed, Error::CborInvalidMapKey, tokenizer.position());
            }
            let key_position = tokenizer.position();
            let key = tokenizer.string8();
            tokenizer.next();
            match key {
                b"id" => {
                    if parsed.call_id.is_some() {
                        return fail(&parsed, Error::CborDuplicateMapKey, key_position);
                    }
                    let Token::Int32(id) = tokenizer.token() else {
                        return fail(&parsed, Error::MessageMustHaveIntegerIdProperty, tokenizer.position());
                    };
                    parsed.call_id = Some(id);
                }
                b"method" => {
                    if !parsed.method.is_empty() {
                        return fail(&parsed, Error::CborDuplicateMapKey, key_position);
                    }
                    if tokenizer.token() != Token::String8 {
                        return fail(&parsed, Error::MessageMustHaveStringMethodProperty, tokenizer.position());
                    }
                    parsed.method = tokenizer.string8();
                }
                b"params" => {
                    if params_seen {
                        return fail(&parsed, Error::CborDuplicateMapKey, key_position);
                    }
                    params_seen = true;
                    match tokenizer.token() {
                        Token::Null => {}
                        Token::Envelope => parsed.params = Some(tokenizer.envelope()),
                        _ => return fail(&parsed, Error::MessageMayHaveObjectParamsProperty, tokenizer.position()),
                    }
                }
                b"sessionId" => {
                    if !parsed.session_id.is_empty() {
                        return fail(&parsed, Error::CborDuplicateMapKey, key_position);
                    }
                    if tokenizer.token() != Token::String8 {
                        return fail(&parsed, Error::MessageMayHaveStringSessionIdProperty, tokenizer.position());
                    }
                    parsed.session_id = tokenizer.string8();
                }
                _ => return fail(&parsed, Error::MessageHasUnknownProperty, key_position),
            }
            tokenizer.next();
        }
        tokenizer.next();
        if parsed.call_id.is_none() {
            return fail(&parsed, Error::MessageMustHaveIntegerIdProperty, tokenizer.position());
        }
        if parsed.method.is_empty() {
            return fail(&parsed, Error::MessageMustHaveStringMethodProperty, tokenizer.position());
        }
        Ok(parsed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    String,
    Bool,
    Int,
}

#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub name: &'static str,
    pub kind: FieldKind,
    pub optional: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    String(String),
    Bool(bool),
    Int(i32),
}

fn failure(path: &[&str], status: Status) -> String {
    let mut message = String::from("Failed to deserialize params");
    for name in path.iter().rev() {
        message.push('.');
        message.push_str(name);
    }
    format!("{message} - {}", status.describe())
}

fn skip_value(tokenizer: &mut Tokenizer<'_>) {
    tokenizer.next();
}

/// Port of crdtp's DeserializerDescriptor for flat parameter objects, which are all the browser
/// endpoint's commands take; sorted lookup and unknown-field skipping behave as they do there.
/// The error is the `data` string of the -32602 reply.
pub fn deserialize_params(params: Option<&[u8]>, fields: &[Field]) -> Result<Vec<Option<FieldValue>>, String> {
    let mut values: Vec<Option<FieldValue>> = vec![None; fields.len()];
    let mandatory = |index: usize| !fields[index].optional;
    let bytes = params.unwrap_or_default();
    let mut tokenizer = Tokenizer::new(bytes);
    if tokenizer.token() == Token::Done && !fields.iter().any(|field| !field.optional) {
        return Ok(values);
    }
    if tokenizer.token() == Token::Envelope {
        tokenizer.enter_envelope();
    }
    if tokenizer.token() != Token::MapStart {
        return Err(failure(&[], tokenizer.status(Error::CborMapStartExpected)));
    }
    tokenizer.next();
    while tokenizer.token() != Token::Stop {
        if tokenizer.token() != Token::String8 {
            return Err(failure(&[], tokenizer.status(Error::CborInvalidMapKey)));
        }
        let key = tokenizer.string8();
        tokenizer.next();
        match fields.iter().position(|field| field.name.as_bytes() == key) {
            None => skip_value(&mut tokenizer),
            Some(index) => {
                let field = fields[index];
                let value = match (field.kind, tokenizer.token()) {
                    (FieldKind::String, Token::String8) => FieldValue::String(String::from_utf8_lossy(tokenizer.string8()).into_owned()),
                    (FieldKind::String, Token::String16) => FieldValue::String(tokenizer.string16()),
                    (FieldKind::String, _) => return Err(failure(&[field.name], tokenizer.status(Error::BindingsStringValueExpected))),
                    (FieldKind::Bool, Token::True) => FieldValue::Bool(true),
                    (FieldKind::Bool, Token::False) => FieldValue::Bool(false),
                    (FieldKind::Bool, _) => return Err(failure(&[field.name], tokenizer.status(Error::BindingsBoolValueExpected))),
                    (FieldKind::Int, Token::Int32(value)) => FieldValue::Int(value),
                    (FieldKind::Int, _) => return Err(failure(&[field.name], tokenizer.status(Error::BindingsInt32ValueExpected))),
                };
                values[index] = Some(value);
                tokenizer.next();
            }
        }
    }
    if let Some(index) = (0..fields.len()).find(|index| mandatory(*index) && values[*index].is_none()) {
        return Err(failure(&[fields[index].name], tokenizer.status(Error::BindingsMandatoryFieldMissing)));
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json_parser::to_cbor;

    type Failure = (Option<i32>, String);

    fn dispatch(json: &str) -> Result<(Option<i32>, Vec<u8>), Failure> {
        let cbor = to_cbor(json.as_bytes()).expect("valid JSON");
        match Dispatchable::parse(&cbor) {
            Ok(parsed) => Ok((parsed.call_id(), parsed.method().to_vec())),
            Err((id, status)) => Err((id, status.error.message().to_owned())),
        }
    }

    #[test]
    fn commands_need_an_integer_id_and_a_string_method() {
        assert_eq!(dispatch(r#"{"id":1,"method":"A.b"}"#), Ok((Some(1), b"A.b".to_vec())));
        assert_eq!(dispatch(r#"{"method":"A.b"}"#).unwrap_err().1, "Message must have integer 'id' property");
        assert_eq!(dispatch(r#"{"id":"a","method":"A.b"}"#).unwrap_err(), (None, "Message must have integer 'id' property".to_owned()));
        assert_eq!(dispatch(r#"{"id":2}"#).unwrap_err(), (Some(2), "Message must have string 'method' property".to_owned()));
        assert_eq!(dispatch(r#"{"id":2,"x":1}"#).unwrap_err().0, Some(2));
        assert_eq!(dispatch("[]").unwrap_err().1, "Message must be an object");
        assert_eq!(dispatch("1").unwrap_err().1, "Message must be an object");
    }

    #[test]
    fn missing_mandatory_params_are_reported_at_the_end_of_the_object() {
        let cbor = to_cbor(br#"{"id":7,"method":"Target.activateTarget","params":{}}"#).expect("valid JSON");
        let parsed = Dispatchable::parse(&cbor).expect("a command");
        let fields = [Field { name: "targetId", kind: FieldKind::String, optional: false }];
        assert_eq!(
            deserialize_params(parsed.params(), &fields),
            Err("Failed to deserialize params.targetId - BINDINGS: mandatory field missing at position 8".to_owned())
        );
    }
}
