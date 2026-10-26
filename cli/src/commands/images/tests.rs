use super::*;

#[test]
fn only_upload_urls_in_the_text_are_found_each_once() {
    let description = "See ![shot](https://uploads.linear.app/a1/b2/c3) and https://example.com/x.png, \
        <https://uploads.linear.app/a1/b2/c3> again, https://uploads.linear.app.evil.com/x, http://uploads.linear.app/insecure., https://uploads.linear.app/f6/g7.";
    let comment = "[file](https://uploads.linear.app/d4/e5?x=1)\\https://uploads.linear.app/ alone";
    assert_eq!(find(&[description, comment]), ["https://uploads.linear.app/a1/b2/c3", "https://uploads.linear.app/f6/g7", "https://uploads.linear.app/d4/e5?x=1"]);
}

#[test]
fn the_token_goes_only_to_the_upload_host() {
    assert!(is_upload("https://uploads.linear.app/a1/b2"));
    for url in [
        "https://uploads.linear.app.evil.com/a",
        "https://uploads.linear.app@evil.com/a",
        "https://uploads.linear.app/a\\@evil.com",
        "https://uploads.linear.app/a b",
        "https://uploads.linear.app/",
        "http://uploads.linear.app/a",
        "https://example.com/https://uploads.linear.app/a",
    ] {
        assert!(!is_upload(url), "{url}");
    }
}

#[test]
fn only_image_types_are_kept() {
    assert_eq!(extension("image/png"), Some("png"));
    assert_eq!(extension("IMAGE/JPEG"), Some("jpg"));
    assert_eq!(extension("text/html"), None);
    assert_eq!(extension("image/svg+xml"), None);
    assert_eq!(extension(""), None);
}

#[test]
fn an_issue_without_uploads_fetches_nothing() {
    let issue = json!({ "identifier": "TW-1", "description": "No pictures.", "comments": { "nodes": [{ "body": "https://example.com/x.png" }] } });
    assert_eq!(fetch(&crate::http::agent(), &issue).unwrap(), Vec::<Value>::new());
}

#[test]
fn a_file_is_named_by_its_url_and_replaced_whole() {
    assert_eq!(name("https://uploads.linear.app/a"), name("https://uploads.linear.app/a"));
    assert_ne!(name("https://uploads.linear.app/a"), name("https://uploads.linear.app/b"));
    assert_eq!(name("https://uploads.linear.app/a").len(), 16);
    let dir = std::env::temp_dir().join(format!("tolkster-images-test-{}", std::process::id()));
    let path = dir.join("x.png");
    write(&dir, &path, b"first").unwrap();
    write(&dir, &path, b"second").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"second");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    fs::remove_dir_all(&dir).unwrap();
}
