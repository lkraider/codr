use herdr_white_noise_bridge::output::permission::{self, Reply};
use serde_json::{Value, json};

fn request() -> Value {
    json!({"id":"per_fixture","sessionID":"ses_fixture","action":"shell","resources":["git commit -m 'Fix bug'"],
        "source":{"type":"tool","messageID":"msg_fixture","id":"call_fixture"},"metadata":{"command":"git commit -m 'Fix bug'"}})
}

#[test]
fn permission_codes_bind_every_request_field_and_never_offer_persistent_approval() {
    let request = request();
    let permission = permission::from_response(&json!({"data":[request]}), "ses_fixture")
        .unwrap()
        .unwrap();
    assert_eq!(permission.token().len(), 17);
    assert!(!permission.text().contains("Always allow"));
    for (field, value) in [
        ("id", json!("per_other")),
        ("resources", json!(["rm file"])),
        ("metadata", json!({"command":"rm file"})),
        (
            "source",
            json!({"type":"tool","messageID":"msg_other","id":"call_fixture"}),
        ),
        ("save", json!(["git *"])),
    ] {
        let mut changed = request.clone();
        changed[field] = value;
        let changed = permission::from_response(&json!({"data":[changed]}), "ses_fixture")
            .unwrap()
            .unwrap();
        assert_ne!(permission.token(), changed.token(), "{field}");
    }
}

#[test]
fn malformed_foreign_ambiguous_or_misleading_permissions_fail_closed() {
    assert!(
        permission::from_response(&json!({"data":[]}), "ses_fixture")
            .unwrap()
            .is_none()
    );
    assert!(
        permission::from_response(&json!({"data":[request(),request()]}), "ses_fixture").is_err()
    );
    for (field, value) in [
        ("id", json!("per_../other")),
        ("sessionID", json!("ses_other")),
        ("source", Value::Null),
        ("resources", json!([])),
        ("resources", json!(["x".repeat(6001)])),
        ("resources", json!(["echo \u{1b}[2J"])),
        ("resources", json!(["echo \u{202e}hidden"])),
        ("resources", json!(["echo ```hidden```"])),
    ] {
        let mut changed = request();
        changed[field] = value;
        assert!(
            permission::from_response(&json!({"data":[changed]}), "ses_fixture").is_err(),
            "{field}"
        );
    }
}

#[test]
fn permission_card_displays_literal_command_metadata_and_all_resources() {
    let mut request = request();
    request["resources"] = json!(["git commit *"]);
    request["metadata"]["command"] = json!("git commit -m '[safe](actual text)'\nprintf done");
    let permission = permission::from_response(&json!({"data":[request]}), "ses_fixture")
        .unwrap()
        .unwrap();
    let text = permission.text();
    assert!(text.contains("```text\n"));
    assert!(text.contains("git commit *"));
    assert!(text.contains("git commit -m '[safe](actual text)'\nprintf done"));
}

#[test]
fn only_explicit_once_and_reject_with_one_code_are_parsed() {
    assert!(matches!(
        permission::parse_reply(" /ALLOW p123 "),
        Some((Reply::Once, "p123"))
    ));
    assert!(matches!(
        permission::parse_reply("/reject p123"),
        Some((Reply::Reject, "p123"))
    ));
    for text in [
        "/allow",
        "/allow always p123",
        "/allow p123 extra",
        "/allow-always p123",
        "Always allow",
        "Allow once",
        "Reject",
    ] {
        assert!(permission::is_reply(text, false));
        assert!(permission::parse_reply(text).is_none());
    }
    assert!(permission::is_reply("2", true));
    assert!(!permission::is_reply("Do the next task", true));
}
