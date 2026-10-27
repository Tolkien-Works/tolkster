use super::*;
use serde_json::json;

#[test]
fn the_accessors_read_what_is_there_and_nothing_else() {
    let data = json!({ "a": { "id": "x", "empty": "", "null": null, "nodes": [1, 2] } });
    assert_eq!(non_empty(&data, "/a/id").as_deref(), Some("x"));
    assert_eq!(non_empty(&data, "/a/empty"), None);
    assert_eq!(nodes(&data, "/a/nodes"), [json!(1), json!(2)]);
    assert!(nodes(&data, "/a/id").is_empty() && nodes(&data, "/missing").is_empty());
}

#[test]
fn str_borrows_only_a_string() {
    let data = json!({ "a": { "id": "x", "n": 1 } });
    assert_eq!(str(&data, "/a/id"), Some("x"));
    assert_eq!(str(&data, "/a/n"), None);
}

#[test]
fn decode_names_a_missing_field_and_never_quotes_a_value() {
    #[derive(serde::Deserialize, Debug)]
    #[allow(dead_code)]
    struct Shape {
        id: String,
        n: Option<u64>,
    }
    assert_eq!(decode::<Shape>(json!({ "id": "x" }), "Linear's answer to Q").unwrap().id, "x");
    assert_eq!(decode::<Shape>(json!({}), "Linear's answer to Q").unwrap_err().to_string(), "Linear's answer to Q didn't decode: it had no `id`");
    let wrong = decode::<Shape>(json!({ "id": "secret text" , "n": "1" }), "Q").unwrap_err().to_string();
    assert!(!wrong.contains("secret") && !wrong.contains('1'), "{wrong}");
    let wrong = decode::<Shape>(json!({ "id": 7 }), "Q").unwrap_err().to_string();
    assert_eq!(wrong, "Q didn't decode: a field had another shape");
}
