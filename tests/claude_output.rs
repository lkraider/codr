use herdr_white_noise_bridge::output::claude;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

const SESSION: &str = "11111111-1111-4111-8111-111111111111";

fn user(id: &str, parent: Option<&str>, content: Value) -> Value {
    json!({"type":"user","uuid":id,"parentUuid":parent,"sessionId":SESSION,"message":{"role":"user","content":content}})
}
fn assistant(id: &str, parent: &str, message: &str, reason: Value, content: Value) -> Value {
    json!({"type":"assistant","uuid":id,"parentUuid":parent,"sessionId":SESSION,"message":{"id":message,"role":"assistant","stop_reason":reason,"content":content}})
}
fn write_transcript(home: &Path, project: &str, rows: &[Value]) -> PathBuf {
    let directory = home.join("projects").join(project);
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{SESSION}.jsonl"));
    let text = rows
        .iter()
        .map(|row| serde_json::to_string(row).unwrap() + "\n")
        .collect::<String>();
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn completed_main_reply_combines_only_its_own_text_blocks() {
    let home = tempfile::tempdir().unwrap();
    write_transcript(
        home.path(),
        "project",
        &[
            user("u", None, json!("A prompt")),
            assistant(
                "tool",
                "u",
                "api-1",
                json!("tool_use"),
                json!([{"type":"text","text":"Not the answer"},{"type":"tool_use","input":{"secret":"tool input"}}]),
            ),
            user(
                "result",
                Some("tool"),
                json!([{"type":"tool_result","content":"Do not forward tool output"}]),
            ),
            assistant(
                "a1",
                "result",
                "api-2",
                Value::Null,
                json!([{"type":"thinking","thinking":"private reasoning"},{"type":"text","text":"Hello"}]),
            ),
            assistant(
                "a2",
                "a1",
                "api-2",
                json!("end_turn"),
                json!([{"type":"text","text":"world"}]),
            ),
            json!({"type":"system","subtype":"turn_duration","uuid":"s","parentUuid":"a2","sessionId":SESSION}),
        ],
    );
    let replies = claude::read(home.path(), SESSION).unwrap();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].text, "Hello\nworld");
    assert_eq!(replies[0].id, "claude:a2");
}

#[test]
fn abandoned_branches_and_subagents_are_not_forwarded() {
    let home = tempfile::tempdir().unwrap();
    let mut side = assistant(
        "side",
        "old",
        "api-side",
        json!("end_turn"),
        json!([{"type":"text","text":"Subagent answer"}]),
    );
    side["isSidechain"] = json!(true);
    write_transcript(
        home.path(),
        "project",
        &[
            user("u", None, json!("Prompt")),
            assistant(
                "old",
                "u",
                "api-old",
                json!("end_turn"),
                json!([{"type":"text","text":"Abandoned answer"}]),
            ),
            assistant(
                "selected",
                "u",
                "api-new",
                json!("end_turn"),
                json!([{"type":"text","text":"Selected answer"}]),
            ),
            json!({"type":"progress","uuid":"progress","parentUuid":"selected","sessionId":SESSION}),
            side,
        ],
    );
    let replies = claude::read(home.path(), SESSION).unwrap();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].text, "Selected answer");
}

#[test]
fn private_descendants_cannot_hide_the_latest_main_reply() {
    for (flag, value) in [
        ("isSidechain", json!(true)),
        ("isMeta", json!(true)),
        ("teamName", json!("team")),
    ] {
        let home = tempfile::tempdir().unwrap();
        let mut hidden = assistant(
            "hidden",
            "main",
            "private-api",
            json!("end_turn"),
            json!([{"type":"text","text":"Do not forward"}]),
        );
        hidden[flag] = value;
        write_transcript(
            home.path(),
            "project",
            &[
                user("u", None, json!("Prompt")),
                assistant(
                    "main",
                    "u",
                    "main-api",
                    json!("end_turn"),
                    json!([{"type":"text","text":"Main answer"}]),
                ),
                hidden,
            ],
        );
        let replies = claude::read(home.path(), SESSION).unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].text, "Main answer");
    }
}

#[test]
fn null_stop_reason_is_not_made_final_by_a_duration_marker_and_can_be_backfilled() {
    let home = tempfile::tempdir().unwrap();
    let pending = assistant(
        "a",
        "u",
        "api-1",
        Value::Null,
        json!([{"type":"text","text":"Pending"}]),
    );
    let path = write_transcript(
        home.path(),
        "project",
        &[
            user("u", None, json!("Prompt")),
            pending.clone(),
            json!({"type":"system","subtype":"turn_duration","uuid":"s","parentUuid":"a","sessionId":SESSION}),
        ],
    );
    assert!(claude::read(home.path(), SESSION).unwrap().is_empty());
    let mut complete = pending;
    complete["message"]["stop_reason"] = json!("end_turn");
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file, "{}", serde_json::to_string(&complete).unwrap()).unwrap();
    file.write_all(b"{\"unfinished\":").unwrap();
    let replies = claude::read(home.path(), SESSION).unwrap();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].id, "claude:a");
}

#[test]
fn unknown_session_ambiguous_files_and_nested_subagents_are_rejected() {
    let home = tempfile::tempdir().unwrap();
    let row = assistant(
        "a",
        "u",
        "api",
        json!("end_turn"),
        json!([{"type":"text","text":"Answer"}]),
    );
    write_transcript(
        home.path(),
        "project",
        &[user("u", None, json!("Prompt")), row.clone()],
    );
    let nested = home
        .path()
        .join("projects/project")
        .join(SESSION)
        .join("subagents");
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        nested.join(format!("{SESSION}.jsonl")),
        serde_json::to_string(&row).unwrap() + "\n",
    )
    .unwrap();
    assert_eq!(claude::read(home.path(), SESSION).unwrap().len(), 1);
    write_transcript(home.path(), "other-project", &[row]);
    assert!(claude::read(home.path(), SESSION).is_err());
    assert!(claude::read(home.path(), "../../credentials").is_err());
}

fn final_row(id: &str, parent: &str, text: &str) -> Value {
    assistant(
        id,
        parent,
        &format!("api-{id}"),
        json!("end_turn"),
        json!([{"type":"text","text":text}]),
    )
}

fn append(path: &Path, rows: &[Value]) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    for row in rows {
        writeln!(file, "{row}").unwrap();
    }
}

#[test]
fn a_transcript_above_128_mib_reads() {
    let home = tempfile::tempdir().unwrap();
    let output = "x".repeat(15 * 1024 * 1024);
    let mut rows = vec![user("u0", None, json!("Prompt"))];
    for index in 1..=9 {
        rows.push(user(
            &format!("u{index}"),
            Some(&format!("u{}", index - 1)),
            json!([{"type":"tool_result","content":output}]),
        ));
    }
    rows.push(final_row("a", "u9", "Answer"));
    let path = write_transcript(home.path(), "project", &rows);
    assert!(fs::metadata(path).unwrap().len() > 128 * 1024 * 1024);
    let replies = claude::read(home.path(), SESSION).unwrap();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].text, "Answer");
}

#[test]
fn a_second_read_parses_only_the_appended_rows() {
    let home = tempfile::tempdir().unwrap();
    let path = write_transcript(
        home.path(),
        "project",
        &[
            user("u0", None, json!("Prompt")),
            final_row("a0", "u0", "First"),
            user("u1", Some("a0"), json!("Next ".repeat(1000))),
        ],
    );
    assert_eq!(claude::read(home.path(), SESSION).unwrap().len(), 1);
    let mut text = fs::read(&path).unwrap();
    text[0] = b'!';
    fs::write(&path, &text).unwrap();
    append(&path, &[final_row("a1", "u1", "Second")]);
    let replies = claude::read(home.path(), SESSION).unwrap();
    assert_eq!(
        replies
            .iter()
            .map(|reply| reply.text.as_str())
            .collect::<Vec<_>>(),
        ["First", "Second"]
    );
}

#[test]
fn a_rewritten_transcript_is_parsed_again() {
    let home = tempfile::tempdir().unwrap();
    let path = write_transcript(
        home.path(),
        "project",
        &[
            user("u0", None, json!("Prompt")),
            final_row("a0", "u0", "One"),
        ],
    );
    assert_eq!(claude::read(home.path(), SESSION).unwrap()[0].text, "One");
    write_transcript(
        home.path(),
        "project",
        &[
            user("u0", None, json!("Prompt")),
            final_row("a0", "u0", "Two plus"),
        ],
    );
    assert_eq!(
        claude::read(home.path(), SESSION).unwrap()[0].text,
        "Two plus"
    );
    let tail = final_row("a1", "u1", "Tail");
    write_transcript(
        home.path(),
        "project",
        &[
            user("u0", None, json!("Prompt")),
            final_row("a0", "u0", "Old"),
            user("u1", Some("a0"), json!("Next ".repeat(1000))),
            tail.clone(),
        ],
    );
    assert_eq!(claude::read(home.path(), SESSION).unwrap().len(), 2);
    let replaced = path.with_extension("new");
    let mut text = fs::read(&path).unwrap();
    let old = text.windows(3).position(|bytes| bytes == b"Old").unwrap();
    text[old..old + 3].copy_from_slice(b"New");
    fs::write(&replaced, &text).unwrap();
    fs::rename(&replaced, &path).unwrap();
    let replies = claude::read(home.path(), SESSION).unwrap();
    assert_eq!(replies[0].text, "New");
}
