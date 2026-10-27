// Decoding what Linear, GitHub, Orca and the relay send into its shape, and reading a JSON
// value by pointer where a shape is open, or is Tolkster's own.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::error::Error;

// `value` as `T`, decoded once at the edge: a field `T` relies on and the answer lacks fails
// here, named with `what` (e.g. "Linear's answer to Closed"), rather than reading as no data
// later. Only a missing field's name is kept from serde's message: a wrong type's quotes the
// value, which may be Linear's or GitHub's text.
pub fn decode<T: DeserializeOwned>(value: Value, what: &str) -> Result<T, Error> {
    serde_json::from_value(value).map_err(|error| undecoded(&error, what))
}

// `decode` of a value that is borrowed, as one passed through to output is.
pub fn decode_ref<T: DeserializeOwned>(value: &Value, what: &str) -> Result<T, Error> {
    T::deserialize(value).map_err(|error| undecoded(&error, what))
}

fn undecoded(error: &serde_json::Error, what: &str) -> Error {
    let message = error.to_string();
    let why = message.strip_prefix("missing field ").map_or_else(|| "a field had another shape".to_string(), |field| format!("it had no {field}"));
    Error::Failed(format!("{what} didn't decode: {why}"))
}

// For what only explains an error (`deserialize_with`, with `#[serde(default)]`): a field of
// another shape, null included, reads as `T`'s default rather than failing the answer, so it
// can't hide the refusal or spent budget the rest of the answer names.
pub fn loose<'de, D: Deserializer<'de>, T: DeserializeOwned + Default>(deserializer: D) -> Result<T, D::Error> {
    Ok(serde_json::from_value(Value::deserialize(deserializer)?).unwrap_or_default())
}

// `loose` for each entry of a list: one of another shape is `T`'s default, and still counts.
pub fn loose_each<'de, D: Deserializer<'de>, T: DeserializeOwned + Default>(deserializer: D) -> Result<Vec<T>, D::Error> {
    let entries: Vec<Value> = loose(deserializer)?;
    Ok(entries.into_iter().map(|entry| serde_json::from_value(entry).unwrap_or_default()).collect())
}

// A string field, borrowed; `None` when it is missing, null or not a string.
pub fn str<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

// A string field that may be missing or empty; empty counts as missing.
pub fn non_empty(value: &Value, pointer: &str) -> Option<String> {
    str(value, pointer).filter(|v| !v.is_empty()).map(String::from)
}

// The array at `pointer`, e.g. a connection's `/nodes`; empty when there is none.
pub fn nodes<'a>(value: &'a Value, pointer: &str) -> &'a [Value] {
    value.pointer(pointer).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
}

#[cfg(test)]
mod tests;
