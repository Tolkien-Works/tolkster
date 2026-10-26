use std::error::Error as _;
use std::io;

use super::*;
use crate::gateway::Failure;

#[test]
fn a_failed_read_says_what_it_was_reading_and_keeps_its_source() {
    let read: Result<(), io::Error> = Err(io::Error::new(io::ErrorKind::NotFound, "no such file"));
    let error = read.context("reading machines.json").unwrap_err();
    assert_eq!(error.to_string(), "reading machines.json: no such file");
    let source = error.source().and_then(|source| source.downcast_ref::<io::Error>()).expect("the io::Error is its source");
    assert_eq!(source.kind(), io::ErrorKind::NotFound);
    // Retried and traced as any other failure, so its exit and retry policy are `Failed`'s.
    assert!(matches!(Failure::of(&error), Failure::Transient));
    assert_eq!(crate::trace::error_kind(&error), "error");
    assert!(!error.later() && !error.orcas());
}

#[test]
fn a_failed_decode_says_what_it_was_decoding() {
    let decoded = serde_json::from_str::<u8>("{").context("config.json").unwrap_err();
    assert!(matches!(&decoded, Error::Failed(message) if message.starts_with("config.json: ")), "{decoded}");
}
