use super::*;

#[test]
fn a_teams_states_and_triage_setting_decode_from_linears_answer() {
    let answer = json!({ "team": { "triageEnabled": false, "states": { "nodes": [
        { "id": "s1", "name": "Backlog", "type": "backlog", "position": 0 },
        { "id": "s2", "name": "In Progress", "type": "started", "position": 1.5 },
        { "id": "s3", "name": "Shelved", "type": "paused", "position": 2 },
    ] } } });
    #[derive(Deserialize)]
    struct Read {
        team: TeamStates,
    }
    let team = decode::<Read>(answer, &answer_to(STATES)).unwrap().team;
    assert!(!team.triage_enabled);
    let kinds: Vec<_> = team.states.nodes.iter().map(|s| (s.id.as_str(), s.state_type, s.position)).collect();
    assert_eq!(kinds, [("s1", crate::linear::StateType::Backlog, 0.0), ("s2", crate::linear::StateType::Started, 1.5), ("s3", crate::linear::StateType::Other, 2.0)]);
}

#[test]
fn a_field_the_code_relies_on_fails_the_read_naming_its_query() {
    let answer = json!({ "team": { "states": { "nodes": [] } } });
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Read {
        team: TeamStates,
    }
    let error = decode::<Read>(answer, &answer_to(STATES)).unwrap_err().to_string();
    assert_eq!(error, "Linear's answer to States didn't decode: it had no `triageEnabled`");
}
