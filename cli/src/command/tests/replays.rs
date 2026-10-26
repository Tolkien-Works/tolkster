use super::*;

// The simulator's replays (test/listen-traces.json, which test/machine-sim.test.ts writes): each
// command the machine sent, carried out on the fakes set up as the world the model carried
// it out against, ends as the model said, and its result has every field the model's has,
// of the same type. A field listen sends beyond those, the machine's parser drops.

fn traces() -> Value {
    serde_json::from_str(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../test/listen-traces.json")).unwrap()).unwrap()
}

// The model's world as the fakes: the issue, open as its worker's is started, its sessions,
// its parent and sub-issues, open, the parent delegated to Tolkster by the owner, the comment
// a `readComment` names, plain or a note's, and in Orca the issue's worktree with its agent, listed working, idle
// (`done`) or waiting at a prompt. An exited agent's terminal is gone. The worktree is clean
// at its PR's head, every commit pushed, with its branch in the primary checkout too, and
// GitHub answers nothing. The issue is Tolkster's, and the repo's conversations checkout is
// recorded.
fn replay_world(command: &Command, world: &Value) -> (fake::Linear, Rc<RefCell<World>>, FakeReader<'static>) {
    let state = match world["state"].as_str() {
        Some("completed") => StateType::Completed,
        Some("cancelled") => StateType::Canceled,
        _ => StateType::Started,
    };
    let branch = command.identifier.to_ascii_lowercase();
    let parent = world["parent"].as_str().map(|id| Parent { id: id.into(), identifier: "TW-90".into(), state_type: StateType::Started, delegate_id: Some(APP.into()), priority: 0, undelegated_at: None, canceled_at: None, updated_at: Some(at(0)), delegated_by: Some(Some(OWNER.into())) });
    let children = world["children"].as_array().unwrap().iter().map(|_| member("TW-91", StateType::Started, 0)).collect();
    let read = Issue { id: command.issue_id.to_string(), identifier: command.identifier.to_string(), branch_name: branch.clone(), state_type: state, parent, children, ..issue() };
    let sessions = world["sessions"].as_array().unwrap().iter().map(|id| Session { id: id.as_str().unwrap().into(), issue: Some(read.clone()), ..session(Vec::new()) }).collect();
    let linear = fake::Linear::with(sessions);
    linear.issues.borrow_mut().push(read);
    let now = match state {
        StateType::Completed => crate::sessions::IssueNow::Completed(at(0)),
        StateType::Canceled => crate::sessions::IssueNow::Cancelled(at(0)),
        _ => crate::sessions::IssueNow::Open,
    };
    linear.tolksters.borrow_mut().insert(command.identifier.to_string(), now);
    if let Ok(Order::ReadComment { comment_id }) = &command.order {
        let kind = match (world["comment"]["kind"].as_str(), world["comment"]["root"].as_str()) {
            (Some("noteReply"), Some(root)) => CommentKind::NoteReply { root: root.into() },
            (Some("noteRoot"), Some(root)) => CommentKind::NoteRoot { root: root.into() },
            _ => CommentKind::Plain,
        };
        let comment = CommentFacts { issue_id: Some(command.issue_id.to_string()), kind, newest_owner_at: world["newestComment"].as_str().map(String::from), state };
        linear.comments.borrow_mut().insert(comment_id.to_string(), comment);
    }
    let head = world["pr"]["head"].as_str().unwrap_or(BASE_SHA);
    let github = FakeReader::new(|_, _| Err(Error::Failed("GitHub isn't there".into())));
    let orca = conversing_world();
    {
        let mut o = orca.borrow_mut();
        o.worktrees.push(Worktree { id: CONVERSATIONS.into(), repo_id: "r1".into(), path: "/wt/tolkster-conversations".into(), branch: None, linked_issue: None, is_main: false, is_local: true, agents: Vec::new(), origin: None });
        o.git.insert("/src/tolkster".into(), fake::Git { head: BASE_SHA.into(), branch: Some("main".into()), branches: vec!["main".into(), branch.clone()], ..fake::Git::default() });
    }
    linear.records.checkouts.borrow_mut().push(crate::state::CheckoutRecord { repo_id: "r1".into(), worktree_id: CONVERSATIONS.into(), at: at(0) });
    if let Some(id) = world["worktree"].as_str() {
        let pane = match world["agent"].as_str() {
            Some("working") => Some("working"),
            Some("idle") => Some("done"),
            Some("waiting") => Some("waiting"),
            _ => None,
        };
        // The model's worktrees are all Tolkster's, each with its record.
        let record = crate::state::WorkerRecord { issue: command.identifier.to_string(), worktree_id: id.into(), session_id: "s0".into(), at: at(0), launch: crate::config::ladder::Launch::new(crate::harnesses::DEFAULT, "claude-opus-5-5", "medium"), rung_from: crate::state::RungFrom::Claim };
        linear.records.workers.borrow_mut().insert(id.into(), record);
        let mut o = orca.borrow_mut();
        let path = id.split_once("::").unwrap().1;
        let agents = pane.map(|state| Pane { key: "tab0:leaf0".into(), state: state.into(), since_ms: Some(0) }).into_iter().collect();
        o.git.insert(path.into(), fake::Git { head: head.into(), branch: Some(branch.clone()), branches: vec![branch.clone()], ..fake::Git::default() });
        o.worktrees.push(Worktree { id: id.into(), repo_id: "r1".into(), path: path.into(), branch: Some(branch), linked_issue: Some(command.identifier.to_string()), is_main: false, is_local: true, agents, origin: Some("acme/tolkster".into()) });
        if pane.is_some() {
            o.terminals.push(crate::orca::Terminal { handle: Some("term_0".into()), worktree_id: Some(id.into()), agent: true, pane: Some("tab0:leaf0".into()) });
        }
    }
    (linear, orca, github)
}

fn replayed(config: &Config, replay: &Value) -> Result<(), String> {
    let command = parse_command(&replay["command"]).ok_or("not a command")??;
    let (linear, world, mut github) = replay_world(&command, &replay["world"]);
    let records = Rc::clone(&linear.records);
    // Args that don't read end it failed, as `carry_out` does. An error is never the model's
    // answer: it would stand for any refusal, or a fake set up wrong.
    let finished = match &command.order {
        Ok(_) => carried_with(config, &linear, &records, &mut fake::Orca::on(&world), &mut github, &command).map_err(|error| format!("error: {error}"))?,
        Err(_) => Finished::failed(),
    };
    let outcome = json!(finished.outcome);
    if outcome != replay["result"] {
        return Err(format!("{outcome} where the model said {}", replay["result"]));
    }
    // Whether a reap left the worktree gone is the model's to say, not only its type, and a
    // turn opens in the recorded checkout, making none.
    if command.kind == Kind::ReapWorktree && finished.detail.get("reaped") != replay["data"].get("reaped") {
        return Err(format!("reaped {:?} where the model said {}", finished.detail.get("reaped"), replay["data"]["reaped"]));
    }
    let ours = Value::Object(finished.detail);
    // A start's two flags are values the machine acts on, so they are compared whole: a worktree
    // there already is `madeWorktree: false`, and its `launched` is whether an agent is there.
    if replay["command"]["kind"] == "startWorker" && outcome == "ok" {
        for flag in ["madeWorktree", "launched"] {
            if replay["data"][flag] != ours[flag] {
                return Err(format!("data.{flag} is {} where the model has {}", ours[flag], replay["data"][flag]));
            }
        }
    }
    let w = world.borrow();
    if command.kind == Kind::StartTurn && (!w.created.is_empty() || w.opened.iter().map(|o| o.0.as_str()).collect::<Vec<_>>() != [CONVERSATIONS]) {
        return Err(format!("the turn made {:?} and opened in {:?}", w.created, w.opened));
    }
    conforms(&replay["data"], &ours, "data")
}

// Every field `model` has, `ours` has, of the same type, each list's first members alike. A
// null in the model's is a value not there, which listen may give or not.
fn conforms(model: &Value, ours: &Value, at: &str) -> Result<(), String> {
    match (model, ours) {
        (Value::Null, _) => Ok(()),
        (Value::Object(model), Value::Object(ours)) => model.iter().try_for_each(|(key, value)| match ours.get(key) {
            Some(theirs) => conforms(value, theirs, &format!("{at}.{key}")),
            None => Err(format!("{at}.{key} is missing")),
        }),
        (Value::Array(model), Value::Array(ours)) => match (model.first(), ours.first()) {
            (Some(model), Some(ours)) => conforms(model, ours, &format!("{at}[0]")),
            _ => Ok(()),
        },
        (Value::Bool(_), Value::Bool(_)) | (Value::Number(_), Value::Number(_)) | (Value::String(_), Value::String(_)) => Ok(()),
        _ => Err(format!("{at} is {ours} where the model has {model}")),
    }
}

#[test]
fn the_simulators_commands_end_on_the_fakes_as_its_model_says() {
    let config = fake::config(&briefs("replay"));
    let (mut failures, mut kinds, mut results) = (Vec::new(), std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
    for trace in traces()["traces"].as_array().unwrap() {
        for (n, replay) in trace["replays"].as_array().unwrap().iter().enumerate() {
            kinds.insert(replay["command"]["kind"].as_str().unwrap().to_string());
            results.insert(replay["result"].as_str().unwrap().to_string());
            if let Err(why) = replayed(&config, replay) {
                failures.push(format!("seed {}, replay {n}, {}: {why}", trace["seed"], replay["command"]["kind"]));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    let replayed = ["closeAgent", "deliverMessage", "observe", "postNotice", "readComment", "readIssue", "reapWorktree", "resumeWorker", "startTurn", "startWorker", "stopWorker"];
    assert_eq!(kinds.iter().map(String::as_str).collect::<Vec<_>>(), replayed);
    assert_eq!(results.iter().map(String::as_str).collect::<Vec<_>>(), ["failed", "gone", "held", "ok"]);
}

// A shape changed on one side alone fails the replay: a field the model has that listen
// doesn't send, or sends as another type.
#[test]
fn a_shape_changed_on_one_side_fails_the_replay() {
    let model = json!({ "worktree": { "id": "repo::C:/w/tw-0", "branch": "tw-0" }, "madeWorktree": true });
    assert!(conforms(&model, &json!({ "worktree": { "id": "r1::/wt/tw-1", "branch": "tw-1", "repo": null }, "madeWorktree": false, "launched": true }), "data").is_ok());
    assert!(conforms(&model, &json!({ "worktree": { "id": "r1::/wt/tw-1" }, "madeWorktree": true }), "data").is_err());
    assert!(conforms(&model, &json!({ "worktree": { "id": "r1::/wt/tw-1", "branch": "tw-1" }, "madeWorktree": "yes" }), "data").is_err());
}
