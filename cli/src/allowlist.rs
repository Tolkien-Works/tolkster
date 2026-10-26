// What a conversation's turn may run of `tolkster`: one fixed table, checked once in
// main.rs, and another for a turn in the thread of Tolkster's comment. Anything outside it exits 2, so a new option is refused until added here.
// The gate guards Tolkster's own interface, not the turn: what else it runs is auto
// mode's (docs/dispatcher.md, rule 1).

use crate::args::{Kind, parse};
use crate::error::Error;

// Where a command names the session it acts in, or for a thread's turn the thread, which
// must be the turn's own.
#[derive(Clone, Copy, PartialEq)]
enum SessionAt {
    // Its first positional: `messages <sessionId>`, `start <sessionId>`, `activity <sessionId> <type>`.
    First,
    // An option: `--session <sessionId>`, or `--thread <commentId>`.
    Flag(&'static str),
    // Nowhere: `help`, `issue show`, `issue list`.
    Nowhere,
}

struct Allowed {
    command: &'static str,
    sub: Option<&'static str>,
    options: &'static [(&'static str, Kind)],
    session: SessionAt,
}

const S: Kind = Kind::String;

// Reading the turn's messages and any issue, answering and reporting progress in its own
// session, and changing issues as Tolkster for its own session. Not `issue update
// --delegate`, which starts code work; not `comment --closed`, which posts in no session of
// the turn's; and nothing else.
const TABLE: &[Allowed] = &[
    Allowed { command: "messages", sub: None, options: &[("after", S)], session: SessionAt::First },
    Allowed { command: "start", sub: None, options: &[], session: SessionAt::First },
    Allowed {
        command: "activity",
        sub: None,
        options: &[("body", S), ("body-file", S), ("state", S), ("action", S), ("parameter", S), ("result", S), ("ephemeral", Kind::Boolean), ("option", Kind::List)],
        session: SessionAt::First,
    },
    Allowed { command: "issue", sub: Some("show"), options: &[("comments", Kind::Boolean), ("children", Kind::Boolean), ("attachments", Kind::Boolean), ("images", Kind::Boolean)], session: SessionAt::Nowhere },
    Allowed { command: "issue", sub: Some("list"), options: LIST, session: SessionAt::Nowhere },
    Allowed {
        command: "issue",
        sub: Some("update"),
        options: &[("session", S), ("title", S), ("description-file", S), ("priority", S), ("estimate", S), ("state", S), ("project", S), ("assignee", S), ("add-label", Kind::List), ("remove-label", Kind::List)],
        session: SessionAt::Flag("session"),
    },
    Allowed {
        command: "issue",
        sub: Some("create"),
        options: CREATE,
        session: SessionAt::Flag("session"),
    },
    Allowed { command: "issue", sub: Some("relate"), options: RELATE, session: SessionAt::Flag("session") },
    Allowed { command: "issue", sub: Some("unrelate"), options: RELATE, session: SessionAt::Flag("session") },
    Allowed { command: "comment", sub: None, options: &[("session", S), ("body", S), ("body-file", S)], session: SessionAt::Flag("session") },
    Allowed { command: "help", sub: None, options: &[], session: SessionAt::Nowhere },
];

const LIST: &[(&str, Kind)] = &[("team", S), ("project", S), ("state", S), ("query", S), ("parent", S), ("limit", S)];

const CREATE: &[(&str, Kind)] = &[
    ("session", S),
    ("title", S),
    ("description-file", S),
    ("parent", S),
    ("team", S),
    ("project", S),
    ("priority", S),
    ("estimate", S),
    ("state", S),
    ("assignee", S),
    ("triage", Kind::Boolean),
    ("label", Kind::List),
    ("blocks", Kind::List),
    ("blocked-by", Kind::List),
    ("related", Kind::List),
    ("duplicate-of", Kind::List),
];

const THREAD_CREATE: &[(&str, Kind)] = &[
    ("thread", S),
    ("title", S),
    ("description-file", S),
    ("parent", S),
    ("team", S),
    ("project", S),
    ("priority", S),
    ("estimate", S),
    ("state", S),
    ("assignee", S),
    ("triage", Kind::Boolean),
    ("label", Kind::List),
    ("blocks", Kind::List),
    ("blocked-by", Kind::List),
    ("related", Kind::List),
    ("duplicate-of", Kind::List),
];

const RELATE: &[(&str, Kind)] = &[("session", S), ("blocks", S), ("blocked-by", S), ("related", S), ("duplicate-of", S)];

const THREAD_RELATE: &[(&str, Kind)] = &[("thread", S), ("blocks", S), ("blocked-by", S), ("related", S), ("duplicate-of", S)];

// Options that read a body, which must come from stdin, `-`: never a file.
const BODIES: [&str; 2] = ["body-file", "description-file"];

// A thread's turn reads its thread's replies and any issue, posts its one reply in its own
// thread, and changes issues as Tolkster for its own thread, which commands/asker.rs scopes
// to the owner's request and `issue update` to the thread's issue. Nothing that acts in a
// session, not `issue update --delegate`, and nothing else.
const THREAD_TABLE: &[Allowed] = &[
    Allowed { command: "messages", sub: None, options: &[("thread", S), ("after", S)], session: SessionAt::Flag("thread") },
    Allowed { command: "issue", sub: Some("show"), options: &[("comments", Kind::Boolean), ("children", Kind::Boolean), ("attachments", Kind::Boolean), ("images", Kind::Boolean)], session: SessionAt::Nowhere },
    Allowed { command: "issue", sub: Some("list"), options: LIST, session: SessionAt::Nowhere },
    Allowed {
        command: "issue",
        sub: Some("update"),
        options: &[("thread", S), ("title", S), ("description-file", S), ("priority", S), ("estimate", S), ("state", S), ("project", S), ("assignee", S), ("add-label", Kind::List), ("remove-label", Kind::List)],
        session: SessionAt::Flag("thread"),
    },
    Allowed {
        command: "issue",
        sub: Some("create"),
        options: THREAD_CREATE,
        session: SessionAt::Flag("thread"),
    },
    Allowed { command: "issue", sub: Some("relate"), options: THREAD_RELATE, session: SessionAt::Flag("thread") },
    Allowed { command: "issue", sub: Some("unrelate"), options: THREAD_RELATE, session: SessionAt::Flag("thread") },
    Allowed { command: "comment", sub: None, options: &[("reply-to", S), ("none", Kind::Boolean), ("body", S), ("body-file", S)], session: SessionAt::Flag("reply-to") },
    Allowed { command: "help", sub: None, options: &[], session: SessionAt::Nowhere },
];

// Refuses `argv` unless the table allows it, in the conversation of session `own`.
// `dispatched` is the words of the command main runs for `argv`, from its command table.
pub fn gate(argv: &[String], dispatched: Option<&[&str]>, own: &str) -> Result<(), Error> {
    check(TABLE, argv, dispatched, own, "a conversation", "tolkster messages, start, activity, issue, comment or help", "its own session")
}

// Refuses `argv` unless the thread table allows it, in the turn in the thread of Tolkster's
// comment `root`.
pub fn gate_thread(argv: &[String], dispatched: Option<&[&str]>, root: &str) -> Result<(), Error> {
    check(THREAD_TABLE, argv, dispatched, root, "a reply's turn", "tolkster messages --thread, issue, comment --reply-to or help", "its own thread")
}

fn check(table: &[Allowed], argv: &[String], dispatched: Option<&[&str]>, own: &str, what: &str, commands: &str, only: &str) -> Result<(), Error> {
    let refuse = |why: String| Err(Error::Usage(format!("in {what}, {why}")));
    let (entry, parsed) = allowed(table, argv, dispatched, what, commands)?;
    let at = match entry.session {
        SessionAt::First => parsed.first(),
        SessionAt::Flag(name) => parsed.non_empty(name),
        SessionAt::Nowhere => Some(own),
    };
    if at != Some(own) {
        return refuse(format!("only {only}: {own}"));
    }
    if let Some(name) = BODIES.iter().find(|name| parsed.string(name).is_some_and(|source| source != "-")) {
        return refuse(format!("--{name} reads stdin, -, never a file"));
    }
    Ok(())
}

// The table's entry for `argv` and its options parsed, whichever session it names.
fn allowed<'t>(table: &'t [Allowed], argv: &[String], dispatched: Option<&[&str]>, what: &str, commands: &str) -> Result<(&'t Allowed, crate::args::Parsed), Error> {
    let refuse = |why: String| Error::Usage(format!("in {what}, {why}"));
    let Some((command, rest)) = argv.split_first() else { return Err(refuse(format!("run one of: {commands}"))) };
    let sub = rest.first().map(String::as_str);
    // The entry must name the very command main dispatches, never one a longer name runs.
    let names = |e: &Allowed| dispatched.is_some_and(|words| words.iter().copied().eq([Some(e.command), e.sub].into_iter().flatten()));
    let Some(entry) = table.iter().find(|e| e.command == command && e.sub.is_none_or(|s| sub == Some(s)) && names(e)) else {
        return Err(refuse(format!("tolkster {command}{} doesn't run", sub.filter(|_| command == "issue").map(|s| format!(" {s}")).unwrap_or_default())));
    };
    let rest = if entry.sub.is_some() { &rest[1..] } else { rest };
    if rest.iter().any(|a| a == "--") {
        return Err(refuse("nothing is passed after --".into()));
    }
    let parsed = parse(rest, entry.options, true).map_err(|error| refuse(error.to_string()))?;
    Ok((entry, parsed))
}

// Whether a turn's table, or with `thread` a thread's, runs `argv`'s command with its
// options and bodies, for a brief's line that leaves its session or thread to the prose.
#[cfg(test)]
pub fn runs(argv: &[String], dispatched: Option<&[&str]>, thread: bool) -> Result<(), Error> {
    let (_, parsed) = if thread { allowed(THREAD_TABLE, argv, dispatched, "a reply's turn", "") } else { allowed(TABLE, argv, dispatched, "a conversation", "") }?;
    match BODIES.iter().find(|name| parsed.string(name).is_some_and(|source| source != "-")) {
        Some(name) => Err(Error::Usage(format!("--{name} reads stdin, -, never a file"))),
        None => Ok(()),
    }
}

// The relations `issue relate` takes, which the table names by hand: kept in step.
#[cfg(test)]
fn relation_names() -> Vec<&'static str> {
    crate::issues::RELATIONS.iter().map(|(name, _)| *name).collect()
}

#[cfg(test)]
mod tests;
