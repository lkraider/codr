use herdr_white_noise_bridge::output::opencode::Reader;
use rusqlite::Connection;
use serde_json::{Value, json};

fn database(path: &std::path::Path) -> Connection {
    let db = Connection::open(path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL;
        CREATE TABLE session(id TEXT PRIMARY KEY);
        CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT);
        CREATE INDEX message_session ON message(session_id,time_created,id);
        CREATE TABLE part(id TEXT PRIMARY KEY,session_id TEXT,message_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT);
        CREATE INDEX part_message ON part(message_id,id);
        CREATE TABLE event(id TEXT PRIMARY KEY,aggregate_id TEXT,seq INTEGER,type TEXT,data TEXT);
        CREATE UNIQUE INDEX event_aggregate ON event(aggregate_id,seq);
        INSERT INTO session VALUES ('ses_fixture');
        INSERT INTO event VALUES ('event-0','ses_fixture',0,'session.created.1','{}');").unwrap();
    db
}

fn message(db: &Connection, id: &str, created: i64, info: Value) {
    db.execute("INSERT INTO message VALUES (?1,'ses_fixture',?2,1,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data", (id,created,info.to_string())).unwrap();
}

fn part(db: &Connection, id: &str, mid: &str, data: Value) {
    db.execute("INSERT INTO part VALUES (?1,'ses_fixture',?2,1,1,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data", (id,mid,data.to_string())).unwrap();
}

fn event(db: &Connection, seq: i64, kind: &str, data: Value) {
    db.execute(
        "INSERT INTO event VALUES (?1,'ses_fixture',?2,?3,?4)",
        (format!("event-{seq}"), seq, kind, data.to_string()),
    )
    .unwrap();
}

fn complete() -> Value {
    json!({"role":"assistant","time":{"completed":1},"finish":"stop"})
}

#[test]
fn attachment_only_database_input_is_complete_local_activity() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("opencode.db");
    let db = database(&path);
    message(&db, "image-user", 1, json!({"role":"user"}));
    part(
        &db,
        "image",
        "image-user",
        json!({"type":"file","mime":"image/png"}),
    );
    let snapshot = Reader::default()
        .read(&path, "ses_fixture")
        .unwrap()
        .snapshot;
    let mut activity = herdr_white_noise_bridge::activity::Activity::default();
    activity.baseline(&[]);
    assert!(activity.observe(&snapshot.history.user_inputs));
    assert!(!activity.unresolved());
}

fn fallback_config(root: &std::path::Path) -> herdr_white_noise_bridge::BridgeConfig {
    use std::os::unix::fs::PermissionsExt;
    let cli = root.join("opencode");
    std::fs::write(&cli, r##"#!/bin/sh
if [ "$1" = "--pure" ]; then exit 2; fi
printf x >> "$(dirname "$0")/cli-calls"
printf '%s\n' '{"info":{"id":"ses_fixture"},"messages":[{"type":"user","id":"user","text":"prompt"},{"type":"assistant","id":"reply","finish":"stop","time":{"completed":1},"content":[{"type":"text","text":"CLI recovered reply"}]}]}'
"##).unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
    serde_json::from_value(json!({
        "phone_npub":"unused", "account_id_hex":"aa".repeat(32),
        "state_path":root.join("state.json"), "herdr_socket":root.join("herdr.sock"),
        "wn_agent_socket":root.join("mdk.sock"), "opencode_bin":cli, "opencode_db":root.join("opencode.db")
    })).unwrap()
}

fn fallback_session() -> herdr_white_noise_bridge::herdr::Session {
    herdr_white_noise_bridge::herdr::Session {
        source: "herdr:opencode".into(),
        agent: "opencode".into(),
        kind: "id".into(),
        value: "ses_fixture".into(),
        home: None,
    }
}

#[tokio::test]
async fn configured_database_recovers_with_cli_when_the_v1_log_or_schema_is_unavailable() {
    for mutation in [
        "DELETE FROM event",
        "DROP TABLE event",
        "ALTER TABLE message RENAME COLUMN data TO newer_data",
        "UPDATE event SET type='message.updated.2'",
    ] {
        let root = tempfile::tempdir().unwrap();
        let config = fallback_config(root.path());
        let db = database(config.opencode_db.as_ref().unwrap());
        db.execute_batch(mutation).unwrap();
        let snapshot =
            herdr_white_noise_bridge::output::read_snapshot(&config, &fallback_session())
                .await
                .unwrap();
        assert_eq!(snapshot.history.finals.len(), 1, "{mutation}");
        assert_eq!(snapshot.history.finals[0].text, "CLI recovered reply");
        assert_eq!(snapshot.history.user_inputs.len(), 1);
        assert_eq!(snapshot.history.user_inputs[0].id, "opencode:user");
        assert_eq!(
            snapshot.history.user_inputs[0].fingerprint,
            herdr_white_noise_bridge::activity::UserInput::new(String::new(), "prompt").fingerprint
        );
    }
}

#[test]
fn database_user_parts_supply_phone_fingerprints_and_exclude_synthetic_input() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("opencode.db");
    let db = database(&path);
    message(&db, "user", 1, json!({"role":"user"}));
    part(
        &db,
        "text",
        "user",
        json!({"type":"text","text":"phone prompt"}),
    );
    message(&db, "synthetic", 2, json!({"role":"user"}));
    part(
        &db,
        "synthetic-text",
        "synthetic",
        json!({"type":"text","text":"generated context","synthetic":true}),
    );
    let snapshot = Reader::default()
        .read(&path, "ses_fixture")
        .unwrap()
        .snapshot;
    assert_eq!(snapshot.history.user_inputs.len(), 1);
    assert_eq!(
        snapshot.history.user_inputs[0].fingerprint,
        herdr_white_noise_bridge::activity::UserInput::new(String::new(), "phone prompt")
            .fingerprint
    );
}

#[tokio::test]
async fn database_access_and_content_errors_never_silently_select_cli() {
    for failure in ["missing", "corrupt", "malformed-message", "missing-session"] {
        let root = tempfile::tempdir().unwrap();
        let config = fallback_config(root.path());
        let path = config.opencode_db.as_ref().unwrap();
        match failure {
            "missing" => {}
            "corrupt" => std::fs::write(path, b"not a sqlite database").unwrap(),
            _ => {
                let db = database(path);
                if failure == "missing-session" {
                    db.execute("DELETE FROM session", []).unwrap();
                } else {
                    message(&db, "broken", 1, json!({"wrong":"message shape"}));
                }
            }
        }
        assert!(
            herdr_white_noise_bridge::output::read_snapshot(&config, &fallback_session())
                .await
                .is_err(),
            "{failure}"
        );
        assert!(!root.path().join("cli-calls").exists(), "{failure}");
    }
}

#[tokio::test]
async fn a_locked_database_is_not_misclassified_as_an_upgrade() {
    let root = tempfile::tempdir().unwrap();
    let config = fallback_config(root.path());
    let db = database(config.opencode_db.as_ref().unwrap());
    db.execute_batch("PRAGMA journal_mode=DELETE; BEGIN EXCLUSIVE")
        .unwrap();
    let result =
        herdr_white_noise_bridge::output::read_snapshot(&config, &fallback_session()).await;
    assert!(result.is_err());
    assert!(!root.path().join("cli-calls").exists());
    assert_eq!(
        herdr_white_noise_bridge::output::read_error_message(&result.err().unwrap()),
        "database is busy or locked"
    );
    db.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn idle_reads_fetch_no_bodies_and_an_in_place_update_fetches_only_its_message() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("opencode.db");
    let db = database(&path);
    for i in 0..1500 {
        let id = format!("msg_{i:04}");
        message(&db, &id, i, complete());
        part(
            &db,
            &format!("part_{i:04}"),
            &id,
            json!({"type":"text","text":"Original answer"}),
        );
    }
    let mut reader = Reader::default();
    let first = reader.read(&path, "ses_fixture").unwrap();
    assert!(first.rebuilt);
    assert_eq!(first.messages_read, 1500);
    assert_eq!(first.parts_read, 1500);
    let idle = reader.read(&path, "ses_fixture").unwrap();
    assert_eq!(
        (idle.events_read, idle.messages_read, idle.parts_read),
        (0, 0, 0)
    );
    // Intentionally leave time_updated unchanged. Sequence-based reads must still
    // detect multiple committed changes within the same timestamp millisecond.
    part(
        &db,
        "part_0007",
        "msg_0007",
        json!({"type":"text","text":"Changed answer"}),
    );
    event(
        &db,
        1,
        "message.part.updated.1",
        json!({"part":{"messageID":"msg_0007"}}),
    );
    let changed = reader.read(&path, "ses_fixture").unwrap();
    assert!(!changed.rebuilt);
    assert_eq!(
        (
            changed.events_read,
            changed.messages_read,
            changed.parts_read
        ),
        (1, 1, 1)
    );
    assert_eq!(changed.snapshot.history.finals.len(), 1500);
    assert_eq!(changed.snapshot.history.finals[7].text, "Changed answer");
}

#[test]
fn streaming_completion_tasks_and_questions_use_the_same_incremental_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("opencode.db");
    let db = database(&path);
    message(&db, "user", 1, json!({"role":"user"}));
    message(
        &db,
        "assistant",
        2,
        json!({"role":"assistant","parentID":"user"}),
    );
    part(
        &db,
        "text",
        "assistant",
        json!({"type":"text","text":"Partial"}),
    );
    part(
        &db,
        "private",
        "assistant",
        json!({"type":"reasoning","text":"Never forward reasoning"}),
    );
    let question = json!({"type":"tool","tool":"question","state":{"status":"running","input":{"questions":[{"question":"Continue?","options":[{"label":"Yes"},{"label":"No"}]}]}}});
    part(&db, "question", "assistant", question);
    part(
        &db,
        "tasks",
        "assistant",
        json!({"type":"tool","tool":"todowrite","state":{"status":"completed","input":{"todos":[{"content":"Run checks","status":"in_progress"}]}}}),
    );
    let mut reader = Reader::default();
    let first = reader.read(&path, "ses_fixture").unwrap();
    assert!(first.snapshot.history.finals.is_empty());
    assert!(first.snapshot.decision.is_some());
    assert!(first.snapshot.tasks.unwrap().text().contains("Run checks"));
    part(
        &db,
        "question",
        "assistant",
        json!({"type":"tool","tool":"question","state":{"status":"completed"}}),
    );
    part(
        &db,
        "text",
        "assistant",
        json!({"type":"text","text":"Complete answer"}),
    );
    message(
        &db,
        "assistant",
        2,
        json!({"role":"assistant","parentID":"user","time":{"completed":1},"finish":"stop"}),
    );
    event(
        &db,
        1,
        "message.part.updated.1",
        json!({"part":{"messageID":"assistant"}}),
    );
    event(
        &db,
        2,
        "message.updated.1",
        json!({"info":{"id":"assistant"}}),
    );
    let updated = reader.read(&path, "ses_fixture").unwrap();
    assert_eq!(updated.messages_read, 1);
    assert!(updated.snapshot.decision.is_none());
    assert_eq!(updated.snapshot.history.finals[0].text, "Complete answer");
    assert_eq!(
        updated.snapshot.history.finals[0].turn.as_deref(),
        Some("opencode:user")
    );
}

#[test]
fn deletes_and_malformed_updates_do_not_leave_stale_content_or_advance_a_failed_cursor() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("opencode.db");
    let db = database(&path);
    message(&db, "answer", 1, complete());
    part(
        &db,
        "text",
        "answer",
        json!({"type":"text","text":"Answer"}),
    );
    let mut reader = Reader::default();
    reader.read(&path, "ses_fixture").unwrap();
    db.execute("UPDATE part SET data='invalid JSON' WHERE id='text'", [])
        .unwrap();
    event(
        &db,
        1,
        "message.part.updated.1",
        json!({"part":{"messageID":"answer"}}),
    );
    assert!(reader.read(&path, "ses_fixture").is_err());
    part(
        &db,
        "text",
        "answer",
        json!({"type":"text","text":"Repaired"}),
    );
    assert_eq!(
        reader
            .read(&path, "ses_fixture")
            .unwrap()
            .snapshot
            .history
            .finals[0]
            .text,
        "Repaired"
    );
    db.execute("DELETE FROM part WHERE id='text'", []).unwrap();
    event(
        &db,
        2,
        "message.part.removed.1",
        json!({"messageID":"answer"}),
    );
    assert!(
        reader
            .read(&path, "ses_fixture")
            .unwrap()
            .snapshot
            .history
            .finals
            .is_empty()
    );
    db.execute("DELETE FROM message WHERE id='answer'", [])
        .unwrap();
    event(&db, 3, "message.removed.1", json!({"messageID":"answer"}));
    assert!(
        reader
            .read(&path, "ses_fixture")
            .unwrap()
            .snapshot
            .history
            .finals
            .is_empty()
    );
}

#[test]
fn wal_reads_observe_committed_changes_without_waiting_for_the_writer() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("opencode.db");
    let db = database(&path);
    message(&db, "answer", 1, complete());
    part(
        &db,
        "text",
        "answer",
        json!({"type":"text","text":"Before"}),
    );
    let mut reader = Reader::default();
    reader.read(&path, "ses_fixture").unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    part(&db, "text", "answer", json!({"type":"text","text":"After"}));
    event(
        &db,
        1,
        "message.part.updated.1",
        json!({"part":{"messageID":"answer"}}),
    );
    let old = reader.read(&path, "ses_fixture").unwrap();
    assert_eq!(old.messages_read, 0);
    assert_eq!(old.snapshot.history.finals[0].text, "Before");
    db.execute_batch("COMMIT").unwrap();
    assert_eq!(
        reader
            .read(&path, "ses_fixture")
            .unwrap()
            .snapshot
            .history
            .finals[0]
            .text,
        "After"
    );
}

#[test]
fn restart_and_replaced_or_gapped_event_logs_rebuild_once() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("opencode.db");
    let db = database(&path);
    message(&db, "answer", 1, complete());
    part(
        &db,
        "text",
        "answer",
        json!({"type":"text","text":"Before"}),
    );
    let mut reader = Reader::default();
    reader.read(&path, "ses_fixture").unwrap();
    let restarted = Reader::default().read(&path, "ses_fixture").unwrap();
    assert!(restarted.rebuilt);
    db.execute("UPDATE event SET id='replacement' WHERE seq=0", [])
        .unwrap();
    part(
        &db,
        "text",
        "answer",
        json!({"type":"text","text":"Replaced"}),
    );
    assert!(reader.read(&path, "ses_fixture").unwrap().rebuilt);
    part(
        &db,
        "text",
        "answer",
        json!({"type":"text","text":"After gap"}),
    );
    event(
        &db,
        2,
        "message.part.updated.1",
        json!({"part":{"messageID":"answer"}}),
    );
    let gap = reader.read(&path, "ses_fixture").unwrap();
    assert!(gap.rebuilt);
    assert_eq!(gap.snapshot.history.finals[0].text, "After gap");
    assert_eq!(reader.read(&path, "ses_fixture").unwrap().messages_read, 0);
}

#[test]
fn missing_databases_sessions_and_unsupported_event_versions_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("missing.db");
    let mut reader = Reader::default();
    assert!(reader.read(&path, "ses_fixture").is_err());
    assert!(!path.exists());
    let db = database(&path);
    assert!(reader.read(&path, "unknown-session").is_err());
    reader.read(&path, "ses_fixture").unwrap();
    event(&db, 1, "message.updated.2", json!({"info":{"id":"answer"}}));
    assert!(reader.read(&path, "ses_fixture").is_err());
    assert!(
        Reader::default().read(&path, "ses_fixture").is_err(),
        "Cold starts must reject unknown event versions"
    );
    db.execute("DELETE FROM event WHERE seq=1", []).unwrap();
    event(&db, 2, "message.updated.2", json!({"info":{"id":"answer"}}));
    assert!(
        reader.read(&path, "ses_fixture").is_err(),
        "A gap must not bypass version checks"
    );
}

#[test]
fn deleting_a_visible_message_removes_its_final_and_keeps_other_messages() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("opencode.db");
    let db = database(&path);
    for (i, id) in ["remove", "keep"].iter().enumerate() {
        message(&db, id, i as i64, complete());
        part(
            &db,
            &format!("{id}-text"),
            id,
            json!({"type":"text","text":id}),
        );
    }
    let mut reader = Reader::default();
    assert_eq!(
        reader
            .read(&path, "ses_fixture")
            .unwrap()
            .snapshot
            .history
            .finals
            .len(),
        2
    );
    db.execute("DELETE FROM message WHERE id='remove'", [])
        .unwrap();
    event(&db, 1, "message.removed.1", json!({"messageID":"remove"}));
    let changed = reader.read(&path, "ses_fixture").unwrap();
    assert_eq!(changed.messages_read, 1);
    assert_eq!(changed.snapshot.history.finals.len(), 1);
    assert_eq!(changed.snapshot.history.finals[0].id, "keep");
}

#[tokio::test]
async fn worker_uses_sqlite_without_a_cli_and_restart_keeps_final_deduplication() {
    use bech32::ToBase32;
    use herdr_white_noise_bridge::{BridgeConfig, bridge::Bridge, herdr::Agent, state::Mapping};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("opencode.db");
    let db = database(&path);
    message(&db, "old", 1, complete());
    part(
        &db,
        "old-text",
        "old",
        json!({"type":"text","text":"Historical answer"}),
    );
    message(&db, "new", 2, complete());
    part(
        &db,
        "new-text",
        "new",
        json!({"type":"text","text":"New answer"}),
    );
    let owner = bech32::encode("npub", [0x11u8; 32].to_base32(), bech32::Variant::Bech32).unwrap();
    let config: BridgeConfig = serde_json::from_value(json!({
        "phone_npub":owner,"account_id_hex":"aa".repeat(32),
        "state_path":root.path().join("state.json"),"herdr_socket":root.path().join("herdr.sock"),
        "wn_agent_socket":root.path().join("mdk.sock"),"opencode_db":path,
        "opencode_bin":root.path().join("must-not-run")
    }))
    .unwrap();
    let agent: Agent = serde_json::from_value(json!({"pane_id":"pane","terminal_id":"terminal","agent":"opencode","agent_status":"idle","state_change_seq":1,
        "agent_session":{"source":"herdr:opencode","agent":"opencode","kind":"id","value":"ses_fixture"}})).unwrap();
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        agent.key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            final_ids: vec!["old".into()],
            output_initialised: true,
            active: true,
            last_agent_status: "idle".into(),
            last_state_change_seq: 1,
            agent_record: Some(agent.clone()),
            ..Mapping::default()
        },
    );
    for _ in 0..100 {
        bridge.poll_sessions(vec![agent.clone()]).await.unwrap();
        if bridge.state.mappings[&agent.key()]
            .final_ids
            .contains(&"new".into())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        bridge.state.mappings[&agent.key()]
            .final_ids
            .contains(&"new".into())
    );
    drop(bridge);
    let mut bridge = Bridge::open(config).unwrap();
    bridge
        .state
        .mappings
        .get_mut(&agent.key())
        .unwrap()
        .last_export_at = 0;
    for _ in 0..100 {
        bridge.poll_sessions(vec![agent.clone()]).await.unwrap();
        if bridge.state.mappings[&agent.key()].last_export_at > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(bridge.state.mappings[&agent.key()].last_export_at > 0);
    assert_eq!(
        bridge
            .state
            .outbox
            .iter()
            .filter(|item| item.text == "New answer")
            .count(),
        1
    );
    assert!(
        !bridge
            .state
            .outbox
            .iter()
            .any(|item| item.text == "Historical answer")
    );
}

#[tokio::test]
#[ignore = "requires CODR_TEST_CONFIG and CODR_TEST_OPENCODE_SESSION; reads local data without printing content"]
async fn installed_database_matches_export_and_reports_incremental_read_cost() {
    let config = herdr_white_noise_bridge::BridgeConfig::load(std::path::Path::new(
        &std::env::var("CODR_TEST_CONFIG").unwrap(),
    ))
    .unwrap();
    let id = std::env::var("CODR_TEST_OPENCODE_SESSION").unwrap();
    let path = std::env::var("CODR_TEST_OPENCODE_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| config.opencode_db.clone().unwrap());
    let mut reader = Reader::default();
    let start = std::time::Instant::now();
    let first = reader.read(&path, &id).unwrap();
    let initial_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut start = std::time::Instant::now();
    let mut idle = reader.read(&path, &id).unwrap();
    let mut idle_ms = start.elapsed().as_secs_f64() * 1000.0;
    for _ in 0..20 {
        if idle.messages_read == 0 {
            break;
        }
        start = std::time::Instant::now();
        idle = reader.read(&path, &id).unwrap();
        idle_ms = start.elapsed().as_secs_f64() * 1000.0;
    }
    assert_eq!(
        idle.messages_read, 0,
        "Session changed during idle measurement; rerun when stable"
    );
    let session = herdr_white_noise_bridge::herdr::Session {
        source: "herdr:opencode".into(),
        agent: "opencode".into(),
        kind: "id".into(),
        value: id.clone(),
        home: None,
    };
    let mut cli = config;
    cli.opencode_db = None;
    let start = std::time::Instant::now();
    let exported = herdr_white_noise_bridge::output::read_snapshot(&cli, &session)
        .await
        .unwrap();
    let export_ms = start.elapsed().as_secs_f64() * 1000.0;
    let finals = |snapshot: &herdr_white_noise_bridge::output::SessionSnapshot| {
        snapshot
            .history
            .finals
            .iter()
            .map(|f| (&f.id, &f.text, &f.turn))
            .map(|(id, text, turn)| (id.clone(), text.clone(), turn.clone()))
            .collect::<Vec<_>>()
    };
    assert!(
        finals(&first.snapshot) == finals(&exported),
        "Final projections differ; inspect locally"
    );
    assert!(
        first.snapshot.tasks.as_ref().map(|t| t.text())
            == exported.tasks.as_ref().map(|t| t.text()),
        "Task projections differ; inspect locally"
    );
    assert!(
        first.snapshot.decision == exported.decision,
        "Decision projections differ; inspect locally"
    );
    println!(
        "{{\"initial_messages\":{},\"initial_ms\":{initial_ms:.2},\"idle_messages\":{},\"idle_parts\":{},\"idle_ms\":{idle_ms:.2},\"cli_export_ms\":{export_ms:.2},\"projection_match\":true}}",
        first.messages_read, idle.messages_read, idle.parts_read
    );
}
