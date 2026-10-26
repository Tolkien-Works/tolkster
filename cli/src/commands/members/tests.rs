use super::*;

#[test]
fn active_users_are_listed_by_name_with_their_ids() {
    let users = json!({ "users": { "nodes": [
        { "id": "u2", "name": "Sam Owner", "displayName": "sam", "active": true, "admin": true, "guest": false },
        { "id": "u3", "name": "Gone", "displayName": "gone", "active": false, "admin": false, "guest": false },
        { "id": "u1", "name": "Alex", "displayName": "alex", "active": true, "admin": false, "guest": true },
    ] } });
    let users: crate::linear::Nodes<User> = crate::json::decode(users["users"].clone(), "Linear's answer to Users").unwrap();
    assert_eq!(
        render(users.nodes),
        [
            json!({ "id": "u1", "name": "Alex", "displayName": "alex", "admin": false, "guest": true }),
            json!({ "id": "u2", "name": "Sam Owner", "displayName": "sam", "admin": true, "guest": false }),
        ]
    );
    assert!(render(Vec::new()).is_empty());
}
