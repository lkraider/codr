//! The transcript cache is one static per process, so this file holds a single test.

use herdr_white_noise_bridge::output::claude;
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn session(number: usize) -> String {
    format!("11111111-1111-4111-8111-{number:012}")
}

fn transcript(home: &Path, session: &str, answer: &str) -> PathBuf {
    let path = home
        .join("projects/project")
        .join(format!("{session}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let rows = [
        json!({"type":"user","uuid":"u1","parentUuid":null,"sessionId":session,"message":{"role":"user","content":"First"}}),
        json!({"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":session,"message":{"id":"api-1","role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":answer}]}}),
        json!({"type":"user","uuid":"u2","parentUuid":"a1","sessionId":session,"message":{"role":"user","content":"Second"}}),
        json!({"type":"assistant","uuid":"a2","parentUuid":"u2","sessionId":session,"message":{"id":"api-2","role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"x".repeat(5000)}]}}),
    ];
    let text: String = rows.iter().map(|row| format!("{row}\n")).collect();
    fs::write(&path, text).unwrap();
    path
}

fn first_answer(home: &Path, session: &str) -> String {
    claude::read(home, session).unwrap()[0].text.clone()
}

/// A cached transcript parses only bytes appended after its last read, so an
/// in-place edit before the 4096-byte tail shows only after a fresh parse.
#[test]
fn the_cache_keeps_32_transcripts_and_evicts_the_least_recently_read() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let paths: Vec<_> = (0..33)
        .map(|number| transcript(home, &session(number), "Old answer"))
        .collect();
    for number in 0..32 {
        assert_eq!(first_answer(home, &session(number)), "Old answer");
    }
    for path in &paths[..2] {
        let text = fs::read_to_string(path).unwrap();
        fs::write(path, text.replace("Old answer", "New answer")).unwrap();
    }
    assert_eq!(first_answer(home, &session(0)), "Old answer");

    assert_eq!(first_answer(home, &session(32)), "Old answer");

    assert_eq!(first_answer(home, &session(1)), "New answer");
    assert_eq!(first_answer(home, &session(0)), "Old answer");
}
