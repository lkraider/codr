use herdr_white_noise_bridge::{BridgeConfig, herdr::Session, output};
use serde_json::json;
use std::{
    fs::{self, File, FileTimes},
    os::unix::fs::PermissionsExt,
    time::{Duration, UNIX_EPOCH},
};

#[test]
fn sync_section_breaks_only_appear_between_messages_and_keep_their_text() {
    let messages = [
        output::ConversationMessage {
            id: "one".into(),
            speaker: "You",
            text: "First paragraph\n\nSecond paragraph".into(),
            timestamp: None,
        },
        output::ConversationMessage {
            id: "two".into(),
            speaker: "Agent",
            text: "Reply 🐇".into(),
            timestamp: None,
        },
        output::ConversationMessage {
            id: "three".into(),
            speaker: "You",
            text: "Follow-up".into(),
            timestamp: None,
        },
    ];
    for count in 0..=3 {
        let history = output::SessionHistory {
            recent_messages: messages[..count].to_vec(),
            ..Default::default()
        };
        let excerpt = history.catch_up();
        if count == 0 {
            assert!(excerpt.is_none());
            continue;
        }
        let excerpt = excerpt.unwrap();
        assert_eq!(excerpt.matches("\n\n---\n\n").count(), count - 1);
        assert!(excerpt.starts_with("**You:**\n"));
        assert!(excerpt.ends_with(&messages[count - 1].text));
        for message in &messages[..count] {
            assert!(excerpt.contains(&format!(
                "**{}:**\n_Date unknown_\n{}",
                message.speaker, message.text
            )));
        }
    }
}

#[tokio::test]
async fn sync_excerpt_keeps_three_messages_in_order_across_all_transcript_readers() {
    let session = "00000000-0000-0000-0000-000000000001";
    for provider in ["opencode-v1", "opencode-v2", "codex", "claude"] {
        let root = tempfile::tempdir().unwrap();
        let config = config(root.path());
        let v1_user = |id, text| json!({"info":{"id":id,"role":"user","time":{"created":0}},"parts":[{"type":"text","text":text}]});
        let v1_answer = |id, text| json!({"info":{"id":id,"role":"assistant","finish":"stop","time":{"completed":1}},"parts":[{"type":"text","text":text}]});
        let v2_user = |id, text| json!({"id":id,"type":"user","text":text,"time":{"created":0}});
        let v2_answer = |id, text| json!({"id":id,"type":"assistant","finish":"stop","time":{"completed":1},"content":[{"type":"text","text":text}]});
        let codex_user = |text| json!({"timestamp":"1970-01-01T00:00:00Z","type":"event_msg","payload":{"type":"user_message","message":text}});
        let codex_answer = |id, text| json!({"timestamp":"1970-01-01T00:00:00Z","type":"event_msg","payload":{"type":"task_complete","turn_id":id,"last_agent_message":text}});
        let claude_user = |id, parent, text| json!({"timestamp":"1970-01-01T00:00:00Z","sessionId":session,"uuid":id,"parentUuid":parent,"type":"user","message":{"content":text}});
        let claude_answer = |id, parent, text| json!({"timestamp":"1970-01-01T00:00:00Z","sessionId":session,"uuid":id,"parentUuid":parent,"type":"assistant","message":{"id":id,"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":text}]}});
        let (path, mut rows, next_user) = match provider {
            "opencode-v1" => (
                root.path().join("data.json"),
                vec![
                    v1_user("old", "Old prompt"),
                    v1_answer("first", "Earlier answer"),
                    v1_user("followup", "Follow-up"),
                    json!({"info":{"id":"tool","role":"assistant","finish":"tool-calls","time":{"completed":1}},"parts":[{"type":"text","text":"Tool output"}]}),
                    v1_answer("last", "Latest answer"),
                    json!({"info":{"id":"stream","role":"assistant","finish":"stop"},"parts":[{"type":"text","text":"Unfinished"}]}),
                    json!({"info":{"id":"summary","role":"user","summary":true},"parts":[{"type":"text","text":"Internal summary"}]}),
                ],
                v1_user("new", "Newest prompt"),
            ),
            "opencode-v2" => (
                root.path().join("data.json"),
                vec![
                    v2_user("old", "Old prompt"),
                    v2_answer("first", "Earlier answer"),
                    v2_user("followup", "Follow-up"),
                    json!({"id":"tool","type":"assistant","finish":"tool-calls","time":{"completed":1},"content":[{"type":"text","text":"Tool output"}]}),
                    v2_answer("last", "Latest answer"),
                    json!({"id":"stream","type":"assistant","content":[{"type":"text","text":"Unfinished"}]}),
                    json!({"id":"synthetic","type":"synthetic","text":"Internal summary"}),
                ],
                v2_user("new", "Newest prompt"),
            ),
            "codex" => (
                config
                    .codex_home
                    .join(format!("sessions/rollout-{session}.jsonl")),
                vec![
                    json!({"type":"session_meta","payload":{"id":session}}),
                    codex_user("Old prompt"),
                    codex_answer("first", "Earlier answer"),
                    codex_user("Follow-up"),
                    json!({"type":"response_item","payload":{"type":"function_call_output","output":"Tool output"}}),
                    codex_answer("last", "Latest answer"),
                    codex_answer("last", "Duplicate answer"),
                    json!({"type":"event_msg","payload":{"type":"agent_message","message":"Unfinished"}}),
                ],
                codex_user("Newest prompt"),
            ),
            _ => (
                config
                    .claude_home
                    .join(format!("projects/project/{session}.jsonl")),
                vec![
                    claude_user("old", "", "Old prompt"),
                    claude_answer("first", "old", "Earlier answer"),
                    claude_user("followup", "first", "Follow-up"),
                    json!({"sessionId":session,"uuid":"tool","parentUuid":"followup","type":"assistant","message":{"id":"tool","role":"assistant","stop_reason":"tool_use","content":[{"type":"text","text":"Tool output"}]}}),
                    claude_answer("last", "tool", "Latest answer"),
                    json!({"sessionId":session,"uuid":"private","parentUuid":"last","type":"user","isMeta":true,"message":{"content":"Internal summary"}}),
                ],
                claude_user("new", "last", "Newest prompt"),
            ),
        };
        let opencode = provider.starts_with("opencode");
        if opencode {
            fs::write(
                &config.opencode_bin,
                "#!/bin/sh\ncat \"$(dirname \"$0\")/data.json\"\n",
            )
            .unwrap();
            fs::set_permissions(&config.opencode_bin, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for additional_user in [false, true] {
            if additional_user {
                rows.push(next_user.clone());
            }
            fs::write(
                &path,
                if opencode {
                    json!({"messages":rows}).to_string()
                } else {
                    rows.iter().map(|row| format!("{row}\n")).collect()
                },
            )
            .unwrap();
            let history = output::read_history(
                &config,
                &Session {
                    source: "test".into(),
                    agent: if opencode { "opencode" } else { provider }.into(),
                    kind: "id".into(),
                    value: session.into(),
                    home: None,
                },
            )
            .await
            .unwrap();
            assert_eq!(history.recent_messages.len(), 3, "{provider}");
            let final_reply = history.finals.last().unwrap();
            let source = history
                .recent_messages
                .iter()
                .find(|message| message.speaker == "Agent" && message.text == final_reply.text)
                .unwrap();
            assert_eq!(
                source.id, final_reply.id,
                "Normal forwarding and sync must use the same source identity for {provider}"
            );
            assert_eq!(
                history.catch_up().as_deref(),
                Some(if additional_user {
                    "**You:**\n_1970-01-01 00:00:00 UTC_\nFollow-up\n\n---\n\n**Agent:**\n_1970-01-01 00:00:00 UTC_\nLatest answer\n\n---\n\n**You:**\n_1970-01-01 00:00:00 UTC_\nNewest prompt"
                } else {
                    "**Agent:**\n_1970-01-01 00:00:00 UTC_\nEarlier answer\n\n---\n\n**You:**\n_1970-01-01 00:00:00 UTC_\nFollow-up\n\n---\n\n**Agent:**\n_1970-01-01 00:00:00 UTC_\nLatest answer"
                }),
                "{provider}"
            );
        }
    }
}

fn config(root: &std::path::Path) -> BridgeConfig {
    serde_json::from_value(json!({
        "phone_npub":"unused", "account_id_hex":"aa", "herdr_socket":root.join("herdr.sock"),
        "wn_agent_socket":root.join("wn.sock"), "state_path":root.join("state.json"),
        "opencode_bin":root.join("opencode"), "codex_home":root.join("codex"),
        "claude_home":root.join("claude")
    }))
    .unwrap()
}

#[test]
fn sync_source_identity_distinguishes_messages_with_the_same_text_and_content_edits() {
    let message = |id: &str, speaker, text: &str| output::ConversationMessage {
        id: id.into(),
        speaker,
        text: text.into(),
        timestamp: None,
    };
    let first = message("first", "Agent", "Hello").key();
    assert_eq!(first, message("first", "Agent", "Hello").key());
    assert_ne!(first, message("second", "Agent", "Hello").key());
    assert_ne!(first, message("first", "You", "Hello").key());
    assert_ne!(first, message("first", "Agent", "Corrected answer").key());
    let mut dated = message("first", "Agent", "Hello");
    dated.timestamp = Some(1_793_189_594);
    assert_eq!(
        first,
        dated.key(),
        "Date labels must not reset existing delivery deduplication"
    );
}

/// Manual long replies keep stable, distinct chunk keys across persistence and retries.
#[test]
fn sync_replies_chunk_utf8_without_loss_or_ordinary_mute_bypass() {
    let mut state = herdr_white_noise_bridge::BridgeState {
        forwarding_enabled: false,
        ..Default::default()
    };
    let history = output::SessionHistory {
        recent_messages: [
            ("Agent", "Earlier answer".into()),
            ("You", "🦄".repeat(20000)),
            ("Agent", "Latest answer".into()),
        ]
        .into_iter()
        .map(|(speaker, text)| output::ConversationMessage {
            id: speaker.into(),
            speaker,
            text,
            timestamp: None,
        })
        .collect(),
        ..Default::default()
    };
    let text = history.catch_up().unwrap();
    state.enqueue_manual("group", &text, "sync:request:group");
    assert_eq!(
        state
            .outbox
            .iter()
            .map(|item| item.text.as_str())
            .collect::<String>(),
        text
    );
    assert!(
        state
            .outbox
            .iter()
            .all(|item| item.manual && item.text.len() <= 30000)
    );
    assert_eq!(
        state
            .outbox
            .iter()
            .map(|item| &item.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        state.outbox.len()
    );
    let saved = state.outbox.clone();
    state.enqueue("group", "must not be queued");
    assert_eq!(state.outbox, saved);
    let restored: herdr_white_noise_bridge::BridgeState =
        serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(restored.outbox, saved);
    let mut legacy = serde_json::to_value(&saved[0]).unwrap();
    legacy.as_object_mut().unwrap().remove("manual");
    let old: herdr_white_noise_bridge::state::Outgoing = serde_json::from_value(legacy).unwrap();
    assert!(!old.manual);
}

/// Both OpenCode export formats use source message clocks, not the export time.
#[tokio::test]
async fn sync_history_uses_opencode_v1_and_v2_source_activity() {
    for data in [
        json!({"messages":[
            {"info":{"id":"old","role":"assistant","time":{"completed":1000},"finish":"stop"},"parts":[{"type":"text","text":"latest reply"}]},
            {"info":{"id":"recent","role":"user","time":{"created":12345000}},"parts":[{"type":"text","text":"new prompt"}]}
        ]}),
        json!({"messages":[
            {"type":"assistant","id":"old","time":{"completed":1000,"streamed":12345000},"finish":"stop","content":[{"type":"text","text":"latest reply"}]},
            {"type":"user","id":"recent","time":{"created":1000},"text":"new prompt"}
        ]}),
        json!({"messages":[
            {"info":{"id":"reply","role":"assistant","time":{"created":1000,"updated":12345000,"completed":1000},"finish":"stop"},"parts":[{"type":"text","text":"latest reply"}]}
        ]}),
    ] {
        let root = tempfile::tempdir().unwrap();
        let config = config(root.path());
        fs::write(root.path().join("data.json"), data.to_string()).unwrap();
        fs::write(
            &config.opencode_bin,
            "#!/bin/sh\ncat \"$(dirname \"$0\")/data.json\"\n",
        )
        .unwrap();
        fs::set_permissions(&config.opencode_bin, fs::Permissions::from_mode(0o700)).unwrap();
        let history = output::read_history(
            &config,
            &Session {
                source: "herdr:opencode".into(),
                agent: "opencode".into(),
                kind: "id".into(),
                value: "ses_test".into(),
                home: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(history.activity_at, Some(12345));
        assert_eq!(history.finals.last().unwrap().text, "latest reply");
    }
}

/// Native logs retain their own modification time even after a fresh bridge read.
#[tokio::test]
async fn sync_history_native_activity_comes_from_the_verified_session_log() {
    let session = "00000000-0000-0000-0000-000000000001";
    for agent in ["codex", "claude"] {
        let root = tempfile::tempdir().unwrap();
        let config = config(root.path());
        let (path, rows) = if agent == "codex" {
            (
                config
                    .codex_home
                    .join(format!("sessions/rollout-{session}.jsonl")),
                vec![
                    json!({"type":"session_meta","payload":{"id":session}}),
                    json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn","last_agent_message":"latest reply"}}),
                ],
            )
        } else {
            (
                config
                    .claude_home
                    .join(format!("projects/project/{session}.jsonl")),
                vec![
                    json!({"sessionId":session,"uuid":"user","type":"user","message":{"content":"prompt"}}),
                    json!({"sessionId":session,"uuid":"answer","parentUuid":"user","type":"assistant","message":{"id":"model","role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"latest reply"}]}}),
                ],
            )
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            rows.iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>(),
        )
        .unwrap();
        File::open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(12345)))
            .unwrap();
        let history = output::read_history(
            &config,
            &Session {
                source: format!("herdr:{agent}"),
                agent: agent.into(),
                kind: "id".into(),
                value: session.into(),
                home: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(history.activity_at, Some(12345));
        assert_eq!(history.finals.last().unwrap().text, "latest reply");
    }
}
