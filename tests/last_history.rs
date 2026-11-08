use herdr_white_noise_bridge::{BridgeConfig, herdr::Session, output};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt};

#[tokio::test]
async fn all_providers_keep_a_bounded_replay_window_without_expanding_recent() {
    for provider in ["opencode-v1", "opencode-v2", "codex", "claude"] {
        let root = tempfile::tempdir().unwrap();
        let config: BridgeConfig = serde_json::from_value(json!({
            "phone_npub":"unused", "account_id_hex":"aa", "herdr_socket":root.path().join("h.sock"),
            "wn_agent_socket":root.path().join("w.sock"), "state_path":root.path().join("state.json"),
            "opencode_bin":root.path().join("opencode"), "codex_home":root.path().join("codex"),
            "claude_home":root.path().join("claude")
        })).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        let mut rows: Vec<Value> = Vec::new();
        if provider == "codex" {
            rows.push(json!({"type":"session_meta","payload":{"id":id}}));
        }
        for i in 0..105 {
            let text = format!("Message{i:03}");
            rows.push(match provider {
                "opencode-v1" => json!({"info":{"id":format!("user{i}"),"role":"user"},"parts":[{"type":"text","text":text}]}),
                "opencode-v2" => json!({"id":format!("user{i}"),"type":"user","text":text}),
                "codex" => json!({"type":"event_msg","payload":{"type":"user_message","message":text}}),
                "claude" => json!({"sessionId":id,"uuid":format!("user{i}"),"parentUuid":if i==0 {None} else {Some(format!("user{}",i-1))},"type":"user","message":{"content":text}}),
                _ => unreachable!(),
            });
        }
        rows.push(match provider {
            "opencode-v1" => json!({"info":{"id":"answer","role":"assistant","finish":"stop","time":{"completed":1}},"parts":[{"type":"text","text":"LatestAgent"}]}),
            "opencode-v2" => json!({"id":"answer","type":"assistant","finish":"stop","time":{"completed":1},"content":[{"type":"text","text":"LatestAgent"}]}),
            "codex" => json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"answer","last_agent_message":"LatestAgent"}}),
            "claude" => json!({"sessionId":id,"uuid":"answer","parentUuid":"user104","type":"assistant","message":{"id":"answer","role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"LatestAgent"}]}}),
            _ => unreachable!(),
        });
        let path = match provider {
            "codex" => config
                .codex_home
                .join(format!("sessions/rollout-{id}.jsonl")),
            "claude" => config
                .claude_home
                .join(format!("projects/project/{id}.jsonl")),
            _ => root.path().join("data.json"),
        };
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
                path,
                rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
            )
            .unwrap();
        }
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
                value: id.into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(history.last_messages.len(), 100, "{provider}");
        assert_eq!(history.recent_messages.len(), 3, "{provider}");
        assert!(history.last(1).unwrap().ends_with("LatestAgent"));
        let four = history.last(4).unwrap();
        for text in ["Message102", "Message103", "Message104", "LatestAgent"] {
            assert!(four.contains(text), "{provider}: {text}");
        }
        assert!(!four.contains("Message101"));
        assert!(history.last(100).unwrap().contains("Message006"));
        assert!(!history.last(100).unwrap().contains("Message005"));
        assert!(history.last(0).is_none());
        assert!(!history.catch_up().unwrap().contains("Message102"));
    }
}
