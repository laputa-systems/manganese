use crate::dispatchable::Dispatchable;
use crate::dispatchable::Field;
use crate::dispatchable::FieldKind;
use crate::dispatchable::FieldValue;
use crate::dispatchable::deserialize_params;
use crate::json::protocol_string;
use crate::json_parser::to_cbor;
use crate::registry::Registry;
use crate::registry::Target;
use crate::url::target_url;

const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const SESSION_NOT_FOUND: i32 = -32001;
const TARGET_NOT_FOUND: &str = "No target with given id found";

/// What `Browser.getVersion` and `/json/version` report about the browser.
#[derive(Clone, Debug)]
pub struct Identity {
    pub protocol_version: String,
    pub product: String,
    pub revision: String,
    pub user_agent: String,
    pub js_version: String,
}

struct Reply {
    call_id: Option<i32>,
    body: Result<String, (i32, String, Option<String>)>,
}

impl Reply {
    fn error(call_id: Option<i32>, code: i32, message: impl Into<String>) -> Self {
        Self { call_id, body: Err((code, message.into(), None)) }
    }

    fn serialize(self) -> Vec<u8> {
        let mut out = String::from("{");
        if let Some(id) = self.call_id {
            out.push_str(&format!("\"id\":{id},"));
        }
        match self.body {
            Ok(result) => out.push_str(&format!("\"result\":{result}")),
            Err((code, message, data)) => {
                out.push_str(&format!("\"error\":{{\"code\":{code},\"message\":"));
                protocol_string(&message, &mut out);
                if let Some(data) = data {
                    out.push_str(",\"data\":");
                    protocol_string(&data, &mut out);
                }
                out.push('}');
            }
        }
        out.push('}');
        out.into_bytes()
    }
}

fn target_id_field() -> [Field; 1] {
    [Field { name: "targetId", kind: FieldKind::String, optional: false }]
}

fn string_field(values: &[Option<FieldValue>], index: usize) -> Option<&str> {
    match values.get(index) {
        Some(Some(FieldValue::String(value))) => Some(value),
        _ => None,
    }
}

fn target_info(target: &Target, context_id: &str) -> String {
    let mut out = String::from("{\"targetId\":");
    protocol_string(&target.id, &mut out);
    out.push_str(",\"type\":");
    protocol_string(target.kind.as_str(), &mut out);
    out.push_str(",\"title\":");
    protocol_string(&target.title, &mut out);
    out.push_str(",\"url\":");
    protocol_string(&target.url, &mut out);
    out.push_str(",\"attached\":false,\"canAccessOpener\":false,\"browserContextId\":");
    protocol_string(context_id, &mut out);
    out.push('}');
    out
}

fn with_params(call_id: i32, params: Option<&[u8]>, fields: &[Field], run: impl FnOnce(&[Option<FieldValue>]) -> Result<String, (i32, String)>) -> Reply {
    match deserialize_params(params, fields) {
        Err(data) => Reply { call_id: Some(call_id), body: Err((INVALID_PARAMS, "Invalid parameters".to_owned(), Some(data))) },
        Ok(values) => match run(&values) {
            Ok(result) => Reply { call_id: Some(call_id), body: Ok(result) },
            Err((code, message)) => Reply::error(Some(call_id), code, message),
        },
    }
}

fn run_command(call_id: i32, method: &str, params: Option<&[u8]>, registry: &mut Registry, identity: &Identity) -> Reply {
    match method {
        "Runtime.runIfWaitingForDebugger" => Reply { call_id: Some(call_id), body: Ok("{}".to_owned()) },
        "Browser.getVersion" => {
            let mut out = String::from("{\"protocolVersion\":");
            protocol_string(&identity.protocol_version, &mut out);
            out.push_str(",\"product\":");
            protocol_string(&identity.product, &mut out);
            out.push_str(",\"revision\":");
            protocol_string(&identity.revision, &mut out);
            out.push_str(",\"userAgent\":");
            protocol_string(&identity.user_agent, &mut out);
            out.push_str(",\"jsVersion\":");
            protocol_string(&identity.js_version, &mut out);
            out.push('}');
            Reply { call_id: Some(call_id), body: Ok(out) }
        }
        "Target.getTargets" => with_params(call_id, params, &[], |_| {
            let infos: Vec<String> = registry.targets().iter().map(|target| target_info(target, registry.context_id())).collect();
            Ok(format!("{{\"targetInfos\":[{}]}}", infos.join(",")))
        }),
        "Target.activateTarget" => with_params(call_id, params, &target_id_field(), |values| {
            let id = string_field(values, 0).unwrap_or_default();
            if registry.activate(id) { Ok("{}".to_owned()) } else { Err((INVALID_PARAMS, TARGET_NOT_FOUND.to_owned())) }
        }),
        "Target.closeTarget" => with_params(call_id, params, &target_id_field(), |values| {
            let id = string_field(values, 0).unwrap_or_default();
            if registry.close(id) { Ok("{\"success\":true}".to_owned()) } else { Err((INVALID_PARAMS, TARGET_NOT_FOUND.to_owned())) }
        }),
        "Target.getTargetInfo" => with_params(call_id, params, &[Field { name: "targetId", kind: FieldKind::String, optional: true }], |values| {
            match string_field(values, 0) {
                Some(id) => match registry.get(id) {
                    Some(target) => Ok(format!("{{\"targetInfo\":{}}}", target_info(target, registry.context_id()))),
                    None => Err((INVALID_PARAMS, TARGET_NOT_FOUND.to_owned())),
                },
                None => {
                    let mut out = String::from("{\"targetInfo\":{\"targetId\":");
                    protocol_string(registry.browser_id(), &mut out);
                    out.push_str(",\"type\":\"browser\",\"title\":\"\",\"url\":\"\",\"attached\":true,\"canAccessOpener\":false}}");
                    Ok(out)
                }
            }
        }),
        "Target.createTarget" => with_params(call_id, params, &[Field { name: "url", kind: FieldKind::String, optional: false }], |values| {
            let url = target_url(string_field(values, 0).unwrap_or_default());
            let target = registry.create(&url);
            let mut out = String::from("{\"targetId\":");
            protocol_string(&target.id, &mut out);
            out.push('}');
            Ok(out)
        }),
        _ => Reply::error(Some(call_id), METHOD_NOT_FOUND, format!("'{method}' wasn't found")),
    }
}

/// Answers one DevTools message with the JSON reply Chromium would send.
pub fn handle_message(message: &[u8], registry: &mut Registry, identity: &Identity) -> Vec<u8> {
    let cbor = match to_cbor(message) {
        Ok(cbor) => cbor,
        Err(status) => return Reply::error(None, PARSE_ERROR, status.describe()).serialize(),
    };
    let dispatchable = match Dispatchable::parse(&cbor) {
        Ok(dispatchable) => dispatchable,
        Err((call_id, status)) => {
            let reply = if status.error.is_message_error() {
                Reply::error(call_id, INVALID_REQUEST, status.error.message())
            } else {
                Reply::error(call_id, PARSE_ERROR, status.describe())
            };
            return reply.serialize();
        }
    };
    let call_id = dispatchable.call_id().expect("a parsed command has an id");
    if !dispatchable.session_id().is_empty() {
        return Reply::error(Some(call_id), SESSION_NOT_FOUND, "Session with given id not found.").serialize();
    }
    let method = String::from_utf8_lossy(dispatchable.method()).into_owned();
    run_command(call_id, &method, dispatchable.params(), registry, identity).serialize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::SequenceIds;
    use crate::registry::TargetKind;

    fn identity() -> Identity {
        Identity {
            protocol_version: "1.3".into(),
            product: "Chrome/1".into(),
            revision: "@r".into(),
            user_agent: "UA".into(),
            js_version: "1".into(),
        }
    }

    fn ask(message: &str) -> String {
        let mut registry = Registry::new(Box::new(SequenceIds::new()), Some("b".into()), vec![Target::new("T1", TargetKind::Page, "about:blank", "about:blank")]);
        String::from_utf8(handle_message(message.as_bytes(), &mut registry, &identity())).expect("JSON is UTF-8")
    }

    #[test]
    fn replies_match_chromiums_text() {
        assert_eq!(ask(r#"{"id":1,"method":"Browser.getVersion"}"#), r#"{"id":1,"result":{"protocolVersion":"1.3","product":"Chrome/1","revision":"@r","userAgent":"UA","jsVersion":"1"}}"#);
        assert_eq!(ask(r#"{"id":2,"method":"Nope.nothing"}"#), r#"{"id":2,"error":{"code":-32601,"message":"'Nope.nothing' wasn't found"}}"#);
        assert_eq!(ask(r#"{"method":"Browser.getVersion"}"#), r#"{"error":{"code":-32600,"message":"Message must have integer 'id' property"}}"#);
        assert_eq!(ask("{not json"), r#"{"error":{"code":-32700,"message":"JSON: string literal expected at position 1"}}"#);
        assert_eq!(
            ask(r#"{"id":7,"method":"Target.activateTarget","params":{}}"#),
            r#"{"id":7,"error":{"code":-32602,"message":"Invalid parameters","data":"Failed to deserialize params.targetId - BINDINGS: mandatory field missing at position 8"}}"#
        );
    }

    #[test]
    fn targets_are_listed_activated_and_closed_by_command() {
        assert!(ask(r#"{"id":1,"method":"Target.getTargets"}"#).contains(r#""targetId":"T1","type":"page""#));
        assert_eq!(ask(r#"{"id":1,"method":"Target.activateTarget","params":{"targetId":"T1"}}"#), r#"{"id":1,"result":{}}"#);
        assert_eq!(ask(r#"{"id":1,"method":"Target.closeTarget","params":{"targetId":"nope"}}"#), r#"{"id":1,"error":{"code":-32602,"message":"No target with given id found"}}"#);
    }
}
