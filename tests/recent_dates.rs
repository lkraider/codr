use herdr_white_noise_bridge::{BridgeConfig, herdr::Session, output};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

fn config(root: &Path) -> BridgeConfig {
    serde_json::from_value(json!({
        "phone_npub":"unused", "account_id_hex":"aa", "herdr_socket":root.join("herdr.sock"),
        "wn_agent_socket":root.join("wn.sock"), "state_path":root.join("state.json"),
        "opencode_bin":root.join("opencode"), "codex_home":root.join("codex"),
        "claude_home":root.join("claude")
    }))
    .unwrap()
}

const SESSION: &str = "00000000-0000-0000-0000-000000000001";

fn transcript(config: &BridgeConfig, provider: &str, known_dates: bool) {
    let user_at = 1_793_099_533_000_u64;
    let agent_at = 1_793_189_594_000_u64;
    let (path, mut rows) = match provider {
        "opencode-v1" => (
            config.opencode_bin.with_file_name("data.json"),
            vec![
                json!({"info":{"id":"user","role":"user","time":{"created":user_at}},"parts":[{"type":"text","text":"Question"}]}),
                json!({"info":{"id":"answer","role":"assistant","finish":"stop","time":{"created":agent_at,"completed":agent_at+5000}},"parts":[{"type":"text","text":"Answer"}]}),
            ],
        ),
        "opencode-v2" => (
            config.opencode_bin.with_file_name("data.json"),
            vec![
                json!({"id":"user","type":"user","text":"Question","time":{"created":user_at}}),
                json!({"id":"answer","type":"assistant","finish":"stop","time":{"created":agent_at,"completed":agent_at+5000},"content":[{"type":"text","text":"Answer"}]}),
            ],
        ),
        "codex" => (
            config
                .codex_home
                .join(format!("sessions/rollout-{SESSION}.jsonl")),
            vec![
                json!({"type":"session_meta","payload":{"id":SESSION}}),
                json!({"timestamp":"2026-10-27T13:12:13+02:00","type":"event_msg","payload":{"type":"user_message","message":"Question"}}),
                json!({"timestamp":"2026-10-28T12:13:14.123Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"answer","last_agent_message":"Answer"}}),
            ],
        ),
        "claude" => (
            config
                .claude_home
                .join(format!("projects/project/{SESSION}.jsonl")),
            vec![
                json!({"timestamp":"2026-10-27T13:12:13+02:00","sessionId":SESSION,"uuid":"user","type":"user","message":{"content":"Question"}}),
                json!({"timestamp":"2026-10-28T12:13:14.123Z","sessionId":SESSION,"uuid":"answer","parentUuid":"user","type":"assistant","message":{"id":"answer","role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"Answer"}]}}),
            ],
        ),
        _ => unreachable!(),
    };
    if !known_dates {
        for row in &mut rows {
            if provider == "opencode-v1" {
                row["info"]["time"] = json!({"completed":u64::MAX});
            } else if provider == "opencode-v2" {
                row["time"] = json!({"completed":u64::MAX});
            } else {
                row["timestamp"] = json!("not a timestamp");
            }
        }
    }
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    if provider.starts_with("opencode") {
        fs::write(&path, json!({"messages":rows}).to_string()).unwrap();
        fs::write(
            &config.opencode_bin,
            "#!/bin/sh\ncat \"$(dirname \"$0\")/data.json\"\n",
        )
        .unwrap();
        fs::set_permissions(&config.opencode_bin, fs::Permissions::from_mode(0o700)).unwrap();
    } else {
        fs::write(
            &path,
            rows.iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>(),
        )
        .unwrap();
    }
}

#[tokio::test]
async fn recent_excerpt_labels_each_sender_with_the_original_source_date_in_utc() {
    for provider in ["opencode-v1", "opencode-v2", "codex", "claude"] {
        let root = tempfile::tempdir().unwrap();
        let config = config(root.path());
        transcript(&config, provider, true);
        let history = output::read_history(
            &config,
            &Session {
                home: None,
                source: "test".into(),
                agent: if provider.starts_with("opencode") {
                    "opencode"
                } else {
                    provider
                }
                .into(),
                kind: "id".into(),
                value: SESSION.into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            history.catch_up().as_deref(),
            Some(
                "**You:**\n_2026-10-27 11:12:13 UTC_\nQuestion\n\n---\n\n**Agent:**\n_2026-10-28 12:13:14 UTC_\nAnswer"
            ),
            "{provider}"
        );
    }
}

#[tokio::test]
async fn recent_excerpt_does_not_invent_dates_for_missing_or_invalid_source_clocks() {
    for provider in ["opencode-v1", "opencode-v2", "codex", "claude"] {
        let root = tempfile::tempdir().unwrap();
        let config = config(root.path());
        transcript(&config, provider, false);
        let history = output::read_history(
            &config,
            &Session {
                home: None,
                source: "test".into(),
                agent: if provider.starts_with("opencode") {
                    "opencode"
                } else {
                    provider
                }
                .into(),
                kind: "id".into(),
                value: SESSION.into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            history.catch_up().as_deref(),
            Some("**You:**\n_Date unknown_\nQuestion\n\n---\n\n**Agent:**\n_Date unknown_\nAnswer"),
            "{provider}"
        );
    }
}

#[tokio::test]
async fn native_excerpts_mark_absent_timestamps_unknown_instead_of_using_file_mtime() {
    for provider in ["codex", "claude"] {
        let root = tempfile::tempdir().unwrap();
        let config = config(root.path());
        transcript(&config, provider, true);
        let path = if provider == "codex" {
            config
                .codex_home
                .join(format!("sessions/rollout-{SESSION}.jsonl"))
        } else {
            config
                .claude_home
                .join(format!("projects/project/{SESSION}.jsonl"))
        };
        let rows: String = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| {
                let mut row: serde_json::Value = serde_json::from_str(line).unwrap();
                row.as_object_mut().unwrap().remove("timestamp");
                format!("{row}\n")
            })
            .collect();
        fs::write(path, rows).unwrap();
        let history = output::read_history(
            &config,
            &Session {
                home: None,
                source: "test".into(),
                agent: provider.into(),
                kind: "id".into(),
                value: SESSION.into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            history.catch_up().as_deref(),
            Some("**You:**\n_Date unknown_\nQuestion\n\n---\n\n**Agent:**\n_Date unknown_\nAnswer"),
            "{provider}"
        );
    }
}

#[tokio::test]
async fn claude_chunked_answer_keeps_its_first_source_timestamp() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    transcript(&config, "claude", true);
    let path = config
        .claude_home
        .join(format!("projects/project/{SESSION}.jsonl"));
    let mut rows: Vec<serde_json::Value> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    rows[1]["parentUuid"] = json!("chunk");
    rows.insert(1, json!({"timestamp":"2026-10-27T11:12:13Z","sessionId":SESSION,"uuid":"chunk","parentUuid":"user","type":"assistant","message":{"id":"answer","role":"assistant","stop_reason":null,"content":[{"type":"text","text":"First fragment."}]}}));
    fs::write(
        path,
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let history = output::read_history(
        &config,
        &Session {
            home: None,
            source: "test".into(),
            agent: "claude".into(),
            kind: "id".into(),
            value: SESSION.into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        history.catch_up().as_deref(),
        Some(
            "**You:**\n_2026-10-27 11:12:13 UTC_\nQuestion\n\n---\n\n**Agent:**\n_2026-10-27 11:12:13 UTC_\nFirst fragment.\nAnswer"
        )
    );
}
