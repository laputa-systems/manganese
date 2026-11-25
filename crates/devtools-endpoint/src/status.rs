/// The crdtp error codes that a DevTools message can produce before it reaches a domain handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    JsonUnprocessedInputRemains,
    JsonStackLimitExceeded,
    JsonNoInput,
    JsonInvalidToken,
    JsonInvalidNumber,
    JsonInvalidString,
    JsonUnexpectedArrayEnd,
    JsonCommaOrArrayEndExpected,
    JsonStringLiteralExpected,
    JsonColonExpected,
    JsonUnexpectedMapEnd,
    JsonCommaOrMapEndExpected,
    JsonValueExpected,
    CborDuplicateMapKey,
    CborInvalidMapKey,
    CborMapStartExpected,
    MessageMustBeAnObject,
    MessageMustHaveIntegerIdProperty,
    MessageMustHaveStringMethodProperty,
    MessageMayHaveStringSessionIdProperty,
    MessageMayHaveObjectParamsProperty,
    MessageHasUnknownProperty,
    BindingsMandatoryFieldMissing,
    BindingsBoolValueExpected,
    BindingsInt32ValueExpected,
    BindingsStringValueExpected,
}

impl Error {
    pub fn message(self) -> &'static str {
        match self {
            Error::JsonUnprocessedInputRemains => "JSON: unprocessed input remains",
            Error::JsonStackLimitExceeded => "JSON: stack limit exceeded",
            Error::JsonNoInput => "JSON: no input",
            Error::JsonInvalidToken => "JSON: invalid token",
            Error::JsonInvalidNumber => "JSON: invalid number",
            Error::JsonInvalidString => "JSON: invalid string",
            Error::JsonUnexpectedArrayEnd => "JSON: unexpected array end",
            Error::JsonCommaOrArrayEndExpected => "JSON: comma or array end expected",
            Error::JsonStringLiteralExpected => "JSON: string literal expected",
            Error::JsonColonExpected => "JSON: colon expected",
            Error::JsonUnexpectedMapEnd => "JSON: unexpected map end",
            Error::JsonCommaOrMapEndExpected => "JSON: comma or map end expected",
            Error::JsonValueExpected => "JSON: value expected",
            Error::CborDuplicateMapKey => "CBOR: duplicate map key",
            Error::CborInvalidMapKey => "CBOR: invalid map key",
            Error::CborMapStartExpected => "CBOR: map start expected",
            Error::MessageMustBeAnObject => "Message must be an object",
            Error::MessageMustHaveIntegerIdProperty => "Message must have integer 'id' property",
            Error::MessageMustHaveStringMethodProperty => "Message must have string 'method' property",
            Error::MessageMayHaveStringSessionIdProperty => "Message may have string 'sessionId' property",
            Error::MessageMayHaveObjectParamsProperty => "Message may have object 'params' property",
            Error::MessageHasUnknownProperty => "Message has property other than 'id', 'method', 'sessionId', 'params'",
            Error::BindingsMandatoryFieldMissing => "BINDINGS: mandatory field missing",
            Error::BindingsBoolValueExpected => "BINDINGS: bool value expected",
            Error::BindingsInt32ValueExpected => "BINDINGS: int32 value expected",
            Error::BindingsStringValueExpected => "BINDINGS: string value expected",
        }
    }

    pub fn is_message_error(self) -> bool {
        matches!(
            self,
            Error::MessageMustBeAnObject
                | Error::MessageMustHaveIntegerIdProperty
                | Error::MessageMustHaveStringMethodProperty
                | Error::MessageMayHaveStringSessionIdProperty
                | Error::MessageMayHaveObjectParamsProperty
                | Error::MessageHasUnknownProperty
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub error: Error,
    pub position: usize,
}

impl Status {
    pub fn describe(&self) -> String {
        format!("{} at position {}", self.error.message(), self.position)
    }
}
