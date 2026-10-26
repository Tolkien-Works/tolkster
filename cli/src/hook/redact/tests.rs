use serde_json::json;

use super::*;

fn scrub() -> Scrub {
    let credentials = json!({ "linearToken": "lin_oauth_abcdef0123456789abcdef", "hostCredential": "env-1:Zq7xV2mN9pL4kR8sT1uW6yB3cD5fG0hJ", "nested": { "short": "ab", "key": "s3cretvalue" } });
    Scrub::new("D:\\source\\worktrees\\tolkster\\tw-1", "C:\\Users\\someone", &["WORKBOX-7".into()], &credentials)
}

#[test]
fn the_worktree_becomes_relative_and_the_home_directory_a_tilde() {
    let s = scrub();
    assert_eq!(s.text("edited D:\\source\\worktrees\\tolkster\\tw-1\\cli\\src\\x.rs", 500), "edited cli\\src\\x.rs");
    assert_eq!(s.text("cd /d/source/worktrees/tolkster/tw-1 && ls", 500), "cd . && ls");
    assert_eq!(s.text("d:/SOURCE/worktrees/tolkster/tw-1/a.rs", 500), "a.rs");
    assert_eq!(s.text("read C:\\Users\\someone\\.tolkster\\config.json", 500), "read ~\\.tolkster\\config.json");
    // A sibling worktree whose name starts the same is not this one, and goes as any other
    // absolute path does.
    assert_eq!(s.text("D:\\source\\worktrees\\tolkster\\tw-12\\a.rs", 500), "…/a.rs");
}

#[test]
fn any_other_absolute_path_keeps_only_its_last_part() {
    let s = scrub();
    assert_eq!(s.text("wrote /tmp/build/out.log, then E:\\private\\notes.txt.", 500), "wrote …/out.log, then …/notes.txt.");
    assert_eq!(s.text("open \\\\fileserver\\share\\x.md", 500), "open …/x.md");
    // A URL, a relative path and a lone slash option are not absolute paths.
    assert_eq!(s.text("see https://linear.app/team/x and cli/src/a.rs, dir /s", 500), "see https://linear.app/team/x and cli/src/a.rs, dir /s");
}

#[test]
fn credentials_secret_shapes_and_host_names_are_scrubbed() {
    let s = scrub();
    let text = s.text("token lin_oauth_abcdef0123456789abcdef on WORKBOX-7 with s3cretvalue and Zq7xV2mN9pL4kR8sT1uW6yB3cD5fG0hJ", 500);
    assert_eq!(text, "token <secret> on <host> with <secret> and <secret>");
    // Shapes no credential holds.
    assert_eq!(s.text("export GH=ghp_1234567890abcdefghij", 500), "export GH=<secret>");
    assert_eq!(s.text("Authorization: Bearer abc.def", 500), "Authorization: <secret> <secret>");
    assert_eq!(s.text("--password=hunter2 api_key: xyz", 500), "--password=<secret> api_key: <secret>");
    // A name that ends in a key word, and a key flag before a space.
    assert_eq!(s.text("CLOUDFLARE_API_TOKEN=abcDEF123_ghiJKL456-mno x-api-key: abc", 500), "CLOUDFLARE_API_TOKEN=<secret> x-api-key: <secret>");
    assert_eq!(s.text("$env:TYPESAFE_API_KEY = 'opaque'; login --token opaque2 --password hunter2", 500), "$env:TYPESAFE_API_KEY = '<secret>'; login --token <secret> --password <secret>");
    assert_eq!(s.text("key A1b2C3d4E5f6G7h8I9j0K1l2M3n4", 500), "key <secret>");
    assert_eq!(s.text("-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----\ndone", 500), "<secret>\ndone");
    // A block whose start was cut off.
    assert_eq!(s.text("MIIE\nabcd\n-----END PRIVATE KEY-----\ndone", 500), "<secret>\ndone");
    // A host's name inside another word, a commit's SHA, a UUID and a branch name stay.
    let kept = "workbox-77 0123456789abcdef0123456789abcdef01234567 ffa12324-2987-4eb5-8540-c7d33ff37d5b show-a-workers-narration-tw-858";
    assert_eq!(s.text(kept, 500), kept);
    assert_eq!(s.text("TOKEN =hunter22", 500), "TOKEN =<secret>");
    // A quoted value, a value after `= `, and a camelCase name.
    assert_eq!(s.text("password=\"hunter2\" --token='abc' TOKEN= value accessToken: v1", 500), "password=\"<secret>\" --token='<secret>' TOKEN= <secret> accessToken: <secret>");
    // The word "token" in prose doesn't take the next word, nor does a word ending in "key".
    assert_eq!(s.text("the token budget is spent, monkey: yes", 500), "the token budget is spent, monkey: yes");
}

#[test]
fn text_is_capped_by_characters() {
    assert_eq!(cap("ab€de", 3), "ab€…");
    assert_eq!(cap("abc", 3), "abc");
    assert_eq!(scrub().text("abcdef", 4), "abcd…");
}
