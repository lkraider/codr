use herdr_white_noise_bridge::{BridgeConfig, herdr::Session, output};

#[tokio::test]
#[ignore = "requires CODR_TEST_CONFIG and CODR_TEST_DECISION_SESSION with a pending OpenCode question; read-only"]
async fn installed_pending_question_is_available_without_answering_it() {
    let config = BridgeConfig::load(std::path::Path::new(
        &std::env::var("CODR_TEST_CONFIG").unwrap(),
    ))
    .unwrap();
    let session = Session {
        source: "herdr:opencode".into(),
        agent: "opencode".into(),
        kind: "id".into(),
        value: std::env::var("CODR_TEST_DECISION_SESSION").unwrap(),
        home: None,
    };
    let decision = output::decision::read(&config, &session)
        .await
        .unwrap()
        .expect("Expected a pending live question");
    assert!(decision.supported());
    assert!(!decision.submitted);
}

#[tokio::test]
#[ignore = "requires CODR_TEST_CONFIG and CODR_TEST_DECISION_SESSION with a visible pending question; read-only"]
async fn installed_pending_question_matches_its_visible_dialog_without_answering_it() {
    use herdr_white_noise_bridge::{herdr, wire::Socket};
    use serde_json::json;
    let config = BridgeConfig::load(std::path::Path::new(
        &std::env::var("CODR_TEST_CONFIG").unwrap(),
    ))
    .unwrap();
    let id = std::env::var("CODR_TEST_DECISION_SESSION").unwrap();
    let socket = Socket {
        path: config.herdr_socket.clone(),
    };
    let agent = herdr::snapshot(&socket)
        .await
        .unwrap()
        .into_iter()
        .find(|agent| {
            agent
                .agent_session
                .as_ref()
                .is_some_and(|session| session.value == id)
        })
        .expect("Session must be live");
    let current = herdr::get(&socket, &agent).await.unwrap();
    assert_eq!(current.agent_status, "blocked");
    let session = current.agent_session.as_ref().unwrap();
    let decision = output::decision::read(&config, session)
        .await
        .unwrap()
        .expect("Expected one pending question");
    let visible = socket.herdr("agent.read", json!({"target":agent.pane_id,"source":"visible","format":"text","lines":300,"strip_ansi":true})).await.unwrap();
    let read = &visible["result"]["read"];
    assert_ne!(read["truncated"], true);
    assert!(
        decision.matches_visible(read["text"].as_str().unwrap()),
        "The live question must match the visible current dialog"
    );
    let after = output::decision::read(&config, session)
        .await
        .unwrap()
        .expect("Question must remain pending");
    assert!(decision.same_request(&after));
    assert!(!after.submitted);
}
