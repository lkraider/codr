use herdr_white_noise_bridge::output::codex;
use std::path::PathBuf;

#[test]
#[ignore = "requires a local Codex rollout and CODR_TEST_CODEX_SESSION"]
fn installed_codex_transcript_yields_completed_replies_without_model_calls() {
    let session = std::env::var("CODR_TEST_CODEX_SESSION").expect("set CODR_TEST_CODEX_SESSION");
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".codex"));
    let replies = codex::read(&home, &session).unwrap();
    assert!(!replies.is_empty());
    println!(
        "Read {} completed Codex replies; transcript text omitted",
        replies.len()
    );
}

#[test]
#[ignore = "requires a Claude transcript and CODR_TEST_CLAUDE_SESSION"]
fn installed_claude_transcript_yields_completed_replies_without_model_calls() {
    let session = std::env::var("CODR_TEST_CLAUDE_SESSION").expect("set CODR_TEST_CLAUDE_SESSION");
    let home = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".claude"));
    let replies = herdr_white_noise_bridge::output::claude::read(&home, &session).unwrap();
    assert!(!replies.is_empty());
    println!(
        "Read {} completed Claude replies; transcript text omitted",
        replies.len()
    );
}

#[tokio::test]
#[ignore = "requires CODR_TEST_CONFIG, CODR_TEST_TASK_AGENT, and CODR_TEST_TASK_SESSION"]
async fn installed_session_yields_structured_tasks_without_model_calls() {
    let path = std::env::var("CODR_TEST_CONFIG").expect("set CODR_TEST_CONFIG");
    let config = herdr_white_noise_bridge::BridgeConfig::load(std::path::Path::new(&path)).unwrap();
    let agent = std::env::var("CODR_TEST_TASK_AGENT").expect("set CODR_TEST_TASK_AGENT");
    let session = herdr_white_noise_bridge::herdr::Session {
        source: format!("herdr:{agent}"),
        agent,
        kind: "id".into(),
        value: std::env::var("CODR_TEST_TASK_SESSION").expect("set CODR_TEST_TASK_SESSION"),
        home: None,
    };
    let tasks = herdr_white_noise_bridge::output::tasks::read_session(&config, &session)
        .await
        .unwrap()
        .expect("The selected session must have a task list");
    assert!(!tasks.items.is_empty());
    println!(
        "Read {} structured tasks; task text omitted",
        tasks.items.len()
    );
}

#[tokio::test]
#[ignore = "requires CODR_TEST_CONFIG and readable active session transcripts"]
async fn installed_active_sessions_have_checkpointable_history() {
    let path = std::env::var("CODR_TEST_CONFIG").expect("set CODR_TEST_CONFIG");
    let config = herdr_white_noise_bridge::BridgeConfig::load(std::path::Path::new(&path)).unwrap();
    let state = herdr_white_noise_bridge::BridgeState::load(&config.state_path).unwrap();
    let started = std::time::Instant::now();
    let limit = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let mut readers = tokio::task::JoinSet::new();
    let mut checked = 0;
    for mapping in state.mappings.values().filter(|mapping| mapping.active) {
        if let Some(session) = mapping
            .agent_record
            .as_ref()
            .and_then(|agent| agent.agent_session.as_ref())
            .filter(|session| herdr_white_noise_bridge::output::supported(&session.agent))
        {
            let config = config.clone();
            let session = session.clone();
            let limit = limit.clone();
            readers.spawn(async move {
                let _permit = limit.acquire_owned().await.unwrap();
                herdr_white_noise_bridge::output::read_history(&config, &session)
                    .await
                    .expect("Active transcript must be readable for a global resume checkpoint");
            });
            checked += 1;
        }
    }
    while let Some(result) = readers.join_next().await {
        result.unwrap();
    }
    assert!(checked > 0);
    println!(
        "Checked {checked} active session histories in {:.1}s; transcript text omitted",
        started.elapsed().as_secs_f64()
    );
}
