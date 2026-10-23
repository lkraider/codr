use bech32::ToBase32;
use herdr_white_noise_bridge::{
    BridgeConfig, BridgeState,
    bridge::{Bridge, now},
    herdr::Agent,
    state::Mapping,
    wire::{Socket, read_line},
};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};
use tokio::{io::AsyncWriteExt, net::UnixListener};

// Synthetic test identity. Never use it as an installation owner.
const HEX: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn phone_npub() -> String {
    bech32::encode("npub", [0x11u8; 32].to_base32(), bech32::Variant::Bech32).unwrap()
}

fn config(dir: &std::path::Path) -> BridgeConfig {
    BridgeConfig {
        phone_npub: phone_npub(),
        herdr_socket: dir.join("herdr.sock"),
        wn_agent_socket: dir.join("wn.sock"),
        account_id_hex: "aa".repeat(32),
        state_path: dir.join("state.json"),
        opencode_bin: dir.join("opencode"),
        opencode_db: None,
        codex_home: dir.join("codex"),
        claude_home: dir.join("claude"),
        edit_responses: false,
        auto_forwarding: false,
        phone_replies_only: false,
        transcribe_command: vec![],
    }
}
fn agent() -> Agent {
    serde_json::from_value(json!({"pane_id":"w1:p1","terminal_id":"term1","tab_id":"w1:t1","agent":"opencode","agent_status":"idle","state_change_seq":1,"agent_session":{"agent":"opencode","source":"herdr:opencode","kind":"id","value":"ses_1"}})).unwrap()
}
fn inbound(sender: &str, id: &str) -> Value {
    json!({"marmot_agent_control":"marmot.agent-control.v2","type":"inbound_message","account_id_hex":"aa".repeat(32),"group_id_hex":"bb".repeat(16),"message":{"message_id_hex":id,"sender":{"account_id_hex":sender},"text":"test prompt","recorded_at":u64::MAX}})
}

async fn peer(
    path: &std::path::Path,
    marmot: bool,
    calls: Arc<Mutex<Vec<Value>>>,
) -> tokio::task::JoinHandle<()> {
    peer_for_agent(path, marmot, calls, agent()).await
}

async fn peer_for_agent(
    path: &std::path::Path,
    marmot: bool,
    calls: Arc<Mutex<Vec<Value>>>,
    observed: Agent,
) -> tokio::task::JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        let mut profile_name = "HDR | historical".to_owned();
        let mut profile_description = String::new();
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(stream);
            let req = read_line(&mut reader).await.unwrap();
            calls.lock().unwrap().push(req.clone());
            let mut response = if marmot {
                match req["type"].as_str().unwrap() {
                    "group_create_capabilities" => {
                        json!({"type":"group_create_capabilities","initial_admins":true})
                    }
                    "group_create" | "group_create_with_admins" => {
                        assert_eq!(req["type"], "group_create_with_admins");
                        assert_eq!(req["members"], json!([phone_npub()]));
                        assert_eq!(req["admins"], json!([phone_npub()]));
                        json!({"type":"group_created","group_id_hex":"bb".repeat(16)})
                    }
                    "send_final" => {
                        json!({"type":"final_sent","message_ids_hex":["cc".repeat(32)]})
                    }
                    "edit_message" => {
                        json!({"type":"final_sent","message_ids_hex":["dd".repeat(32)]})
                    }
                    "group_info" => {
                        json!({"type":"group_info","subject":profile_name,"description":profile_description})
                    }
                    "group_profile_update" => {
                        profile_name = req["name"].as_str().unwrap().into();
                        profile_description = req["description"].as_str().unwrap().into();
                        json!({"type":"group_profile_updated","message_ids_hex":["ee".repeat(32)]})
                    }
                    "download_media" => {
                        let path = req["media"]["test_path"].as_str().unwrap();
                        json!({"type":"media_downloaded","path":path,"media_type":req["media"]["media_type"],
                            "size_bytes":std::fs::metadata(path).unwrap().len()})
                    }
                    "timeline_message_get" => {
                        json!({"type":"timeline_message", "account_id_hex":"aa".repeat(32),
                        "group_id_hex":req["group_id_hex"],"message_id_hex":req["message_id_hex"],
                        "message":{"message_id_hex":req["message_id_hex"],"availability":"available","kind":9,
                            "sender":{"account_id_hex":HEX},"text":"", "attachments":[{"media_type":"audio/ogg","file_name":"voice.ogg"}]}})
                    }
                    _ => panic!("unexpected request"),
                }
            } else {
                match req["method"].as_str().unwrap() {
                    "session.snapshot" => {
                        json!({"result":{"snapshot":{"agents":[observed],"tabs":[{"tab_id":observed.tab_id,"label":"Review","number":1}]}}})
                    }
                    "agent.get" => json!({"result":{"agent":observed}}),
                    "agent.prompt" => json!({"result":{"type":"agent_prompted","agent":observed}}),
                    "agent.read" => json!({"result":{"read":{"text":"Allow this tool?"}}}),
                    _ => panic!("unexpected method"),
                }
            };
            response["id"] = req["id"].clone();
            if marmot {
                response["marmot_agent_control"] = json!("marmot.agent-control.v2");
            }
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            reader.get_mut().write_all(&bytes).await.unwrap();
        }
    })
}

fn export_script(path: &std::path::Path, text: &str, id: &str) {
    let data = json!({"messages":[{"info":{"id":id,"role":"assistant","time":{"completed":1},"finish":"stop"},"parts":[{"type":"text","text":text}]}]});
    std::fs::write(path, format!("#!/bin/sh\nprintf '%s' '{}'\n", data)).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn input_history(
    config: &BridgeConfig,
    session: &str,
    inputs: &[(&str, &str)],
    reply: Option<(&str, &str)>,
) {
    let mut messages: Vec<_> = inputs.iter().map(|(id, text)| json!({"info":{"id":id,"role":"user"},"parts":[{"type":"text","text":text}]})).collect();
    if let Some((id, text)) = reply {
        messages.push(json!({"info":{"id":id,"role":"assistant","parentID":inputs.last().unwrap().0,"finish":"stop","time":{"completed":1}},"parts":[{"type":"text","text":text}]}));
    }
    std::fs::write(
        config
            .opencode_bin
            .with_file_name(format!("{session}.json")),
        json!({"messages":messages}).to_string(),
    )
    .unwrap();
    std::fs::write(
        &config.opencode_bin,
        "#!/bin/sh\ncat \"$(dirname \"$0\")/$3.json\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn activity_mapping(observed: Agent, group: &str) -> Mapping {
    Mapping {
        group_id_hex: Some(group.into()),
        active: true,
        output_initialised: true,
        last_agent_status: "idle".into(),
        last_state_change_seq: 1,
        agent_record: Some(observed),
        ..Mapping::default()
    }
}

#[tokio::test]
async fn incomplete_input_in_an_ended_session_mutes_but_does_not_block_a_phone_wake() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.auto_forwarding = true;
    let first = agent();
    let mut other = first.clone();
    other.agent_session.as_mut().unwrap().value = "ses_other".into();
    other.pane_id = "w1:p2".into();
    other.terminal_id = "term2".into();
    input_history(&config, "ses_1", &[], None);
    input_history(&config, "ses_other", &[], None);
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        first.key(),
        activity_mapping(first.clone(), &"bb".repeat(16)),
    );
    bridge.state.mappings.insert(
        other.key(),
        activity_mapping(other.clone(), &"dd".repeat(16)),
    );
    bridge
        .reconcile(vec![first.clone(), other.clone()])
        .await
        .unwrap();
    std::fs::write(
        config.opencode_bin.with_file_name("ses_1.json"),
        json!({"messages":[{"info":{"id":"incomplete","role":"user"},"parts":[]}]}).to_string(),
    )
    .unwrap();
    bridge.reconcile(vec![other.clone()]).await.unwrap();
    assert!(!bridge.state.forwarding_enabled);
    assert!(!bridge.state.mappings[&first.key()].active);
    assert!(!bridge.state.mappings[&first.key()].activity.unresolved());
    drop(bridge);
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer_for_agent(&config.herdr_socket, false, calls.clone(), other.clone()).await;
    let mut reopened = Bridge::open(config).unwrap();
    reopened.reconcile(vec![other]).await.unwrap();
    let mut event = inbound(HEX, "wake-after-end");
    event["group_id_hex"] = json!("dd".repeat(16));
    reopened.inbound(&event).await.unwrap();
    assert!(reopened.state.forwarding_enabled);
    reopened
        .state
        .enqueue(&"dd".repeat(16), "Delivery is no longer blocked");
    reopened.flush().await.unwrap();
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call["text"] == "Delivery is no longer blocked")
    );
    mdk.abort();
    herdr.abort();
}

#[tokio::test]
async fn a_local_user_message_mutes_every_group_and_drops_queued_output() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.auto_forwarding = true;
    let first = agent();
    let mut other = first.clone();
    other.agent_session.as_mut().unwrap().value = "ses_other".into();
    input_history(&config, "ses_1", &[("old", "Historical prompt")], None);
    input_history(
        &config,
        "ses_other",
        &[("old-other", "Other historical prompt")],
        None,
    );
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        first.key(),
        activity_mapping(first.clone(), &"bb".repeat(16)),
    );
    bridge.state.mappings.insert(
        other.key(),
        activity_mapping(other.clone(), &"dd".repeat(16)),
    );
    bridge
        .reconcile(vec![first.clone(), other.clone()])
        .await
        .unwrap();
    assert!(
        bridge.state.forwarding_enabled,
        "Initial history is only a baseline"
    );
    bridge
        .state
        .enqueue(&"dd".repeat(16), "Queued output from the other group");
    input_history(
        &config,
        "ses_1",
        &[
            ("old", "Historical prompt"),
            ("local", "Work entered in HERDR"),
        ],
        Some(("answer", "Local work must not be forwarded")),
    );
    let mut changed = first.clone();
    changed.state_change_seq = 2;
    bridge.reconcile(vec![changed, other]).await.unwrap();
    assert!(!bridge.state.forwarding_enabled);
    assert!(bridge.state.outbox.is_empty());
    assert!(
        !herdr_white_noise_bridge::forwarding::load(&config.state_path)
            .unwrap()
            .enabled
    );
    drop(bridge);
    assert!(!Bridge::open(config).unwrap().state.forwarding_enabled);
}

#[tokio::test]
async fn phone_input_wakes_all_groups_and_its_native_echo_does_not_mute_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.auto_forwarding = true;
    let first = agent();
    let mut other = first.clone();
    other.agent_session.as_mut().unwrap().value = "ses_other".into();
    input_history(&config, "ses_1", &[("old", "Historical prompt")], None);
    input_history(&config, "ses_other", &[], None);
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer(&config.herdr_socket, false, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        first.key(),
        activity_mapping(first.clone(), &"bb".repeat(16)),
    );
    bridge.state.mappings.insert(
        other.key(),
        activity_mapping(other.clone(), &"dd".repeat(16)),
    );
    bridge
        .reconcile(vec![first.clone(), other.clone()])
        .await
        .unwrap();
    input_history(
        &config,
        "ses_1",
        &[("old", "Historical prompt"), ("local", "Local prompt")],
        None,
    );
    bridge
        .reconcile(vec![first.clone(), other.clone()])
        .await
        .unwrap();
    assert!(!bridge.state.forwarding_enabled);
    let event = inbound(HEX, "phone-handoff");
    bridge.inbound(&event).await.unwrap();
    assert!(bridge.state.forwarding_enabled);
    drop(bridge);
    let transcript = config.opencode_bin.with_file_name("ses_1.json");
    let mut incomplete: Value =
        serde_json::from_slice(&std::fs::read(&transcript).unwrap()).unwrap();
    incomplete["messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"info":{"id":"phone","role":"user"},"parts":[]}));
    std::fs::write(&transcript, incomplete.to_string()).unwrap();
    let mut reopened = Bridge::open(config.clone()).unwrap();
    reopened
        .reconcile(vec![first.clone(), other.clone()])
        .await
        .unwrap();
    assert!(reopened.state.forwarding_enabled);
    assert!(reopened.state.mappings[&first.key()].activity.unresolved());
    reopened
        .state
        .enqueue(&"dd".repeat(16), "Hold while user text is incomplete");
    let before_flush = calls.lock().unwrap().len();
    reopened.flush().await.unwrap();
    assert_eq!(calls.lock().unwrap().len(), before_flush);
    input_history(
        &config,
        "ses_1",
        &[
            ("old", "Historical prompt"),
            ("local", "Local prompt"),
            ("phone", "test prompt"),
        ],
        Some(("phone-reply", "Reply to the phone")),
    );
    let mut changed = first.clone();
    changed.state_change_seq = 3;
    reopened
        .reconcile(vec![changed.clone(), other.clone()])
        .await
        .unwrap();
    assert!(reopened.state.forwarding_enabled);
    reopened.inbound(&event).await.unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call["method"] == "agent.prompt")
            .count(),
        1
    );
    assert!(
        reopened
            .state
            .outbox
            .iter()
            .any(|item| item.text == "Reply to the phone")
    );
    // The same text typed locally later is not another phone-origin prompt.
    input_history(
        &config,
        "ses_1",
        &[
            ("old", "Historical prompt"),
            ("local", "Local prompt"),
            ("phone", "test prompt"),
            ("local-again", "test prompt"),
        ],
        None,
    );
    changed.state_change_seq = 4;
    reopened.reconcile(vec![changed, other]).await.unwrap();
    assert!(!reopened.state.forwarding_enabled);
    mdk.abort();
    herdr.abort();
}

#[tokio::test]
async fn background_reads_deliver_a_fast_session_and_accept_input_while_another_is_slow() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let release = root.path().join("release");
    let started = root.path().join("slow-started");
    let data = json!({"messages":[{"info":{"id":"new-final","role":"assistant","time":{"completed":1},"finish":"stop"},"parts":[{"type":"text","text":"Fast answer"}]}]});
    std::fs::write(&config.opencode_bin, format!("#!/bin/sh\nif [ \"$3\" = ses_slow ]; then touch {started:?}; while [ ! -e {release:?} ]; do sleep 0.01; done; fi\nprintf '%s' '{data}'\n")).unwrap();
    std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer(&config.herdr_socket, false, calls.clone()).await;
    let mut bridge = Bridge::open(config).unwrap();
    let fast = agent();
    let mut slow = fast.clone();
    slow.agent_session.as_mut().unwrap().value = "ses_slow".into();
    slow.pane_id = "w1:p2".into();
    for (a, group) in [(&fast, "bb".repeat(16)), (&slow, "dd".repeat(16))] {
        bridge.state.mappings.insert(
            a.key(),
            Mapping {
                group_id_hex: Some(group),
                output_initialised: true,
                active: true,
                agent_record: Some(a.clone()),
                last_agent_status: "idle".into(),
                last_state_change_seq: 1,
                ..Mapping::default()
            },
        );
    }
    let agents = vec![slow, fast.clone()];
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        bridge.poll_sessions(agents.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    for _ in 0..100 {
        bridge.poll_sessions(agents.clone()).await.unwrap();
        if started.exists()
            && bridge
                .state
                .outbox
                .iter()
                .any(|item| item.group == "bb".repeat(16) && item.text == "Fast answer")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(started.exists(), "The slow export must still be in flight");
    assert!(
        bridge
            .state
            .outbox
            .iter()
            .any(|item| item.group == "bb".repeat(16) && item.text == "Fast answer")
    );
    bridge.flush().await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        bridge.inbound(&inbound(HEX, "phone-during-export")),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        bridge.state.mappings[&fast.key()]
            .seen_messages
            .contains(&"phone-during-export".into())
    );
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call["method"] == "agent.prompt")
            .count(),
        1
    );
    std::fs::write(release, "").unwrap();
    mdk.abort();
    herdr.abort();
}

#[tokio::test]
async fn completion_wakes_keep_uncertain_sessions_active_and_do_not_restart_working_exports() {
    let root = tempfile::tempdir().unwrap();
    let config = editable_config(root.path());
    let starts = root.path().join("starts");
    std::fs::write(
        &config.opencode_bin,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$3\" >> {starts:?}\nprintf '%s' '{{\"messages\":[]}}'\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut bridge = Bridge::open(config).unwrap();
    let mut uncertain = agent();
    let mut agents = Vec::new();
    for index in 0..3 {
        let mut a = agent();
        a.pane_id = format!("w1:p{index}");
        a.terminal_id = format!("term{index}");
        a.agent_session.as_mut().unwrap().value = format!("ses_{index}");
        a.agent_status = "working".into();
        bridge.state.mappings.insert(
            a.key(),
            Mapping {
                group_id_hex: Some(format!("{index:032x}")),
                output_initialised: true,
                active: true,
                agent_record: Some(a.clone()),
                last_agent_status: "working".into(),
                last_state_change_seq: 1,
                ..Mapping::default()
            },
        );
        if index == 0 {
            uncertain = a.clone();
            a.agent_session = None;
        }
        agents.push(a);
    }
    bridge.poll_sessions(agents.clone()).await.unwrap();
    for _ in 0..100 {
        bridge.poll_ready_sessions().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(bridge.state.mappings[&uncertain.key()].active);
    assert!(
        !bridge
            .state
            .outbox
            .iter()
            .any(|item| item.text.starts_with("> ended"))
    );
    assert_eq!(
        std::fs::read_to_string(&starts).unwrap().lines().count(),
        2,
        "Read completion must not trigger the next export before a scheduled poll"
    );
}

#[tokio::test]
async fn an_ended_session_requires_a_post_exit_snapshot_before_it_is_closed() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let source = root.path().join("export.json");
    let started = root.path().join("started");
    let release = root.path().join("release");
    std::fs::write(&source, "{\"messages\":[]}").unwrap();
    std::fs::write(&config.opencode_bin, format!("#!/bin/sh\ndata=$(cat {source:?})\ntouch {started:?}\nwhile [ ! -e {release:?} ]; do sleep 0.01; done\nprintf '%s' \"$data\"\n")).unwrap();
    std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut bridge = Bridge::open(config).unwrap();
    let a = agent();
    bridge.state.mappings.insert(
        a.key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            output_initialised: true,
            active: true,
            agent_record: Some(a.clone()),
            last_agent_status: "idle".into(),
            last_state_change_seq: 1,
            ..Mapping::default()
        },
    );
    bridge.poll_sessions(vec![a.clone()]).await.unwrap();
    for _ in 0..100 {
        if started.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(started.exists());
    std::fs::write(&source, json!({"messages":[{"info":{"id":"last-final","role":"assistant","time":{"completed":1},"finish":"stop"},"parts":[{"type":"text","text":"Final after exit"}]}]}).to_string()).unwrap();
    bridge.poll_sessions(vec![]).await.unwrap();
    std::fs::write(release, "").unwrap();
    for _ in 0..100 {
        bridge.poll_sessions(vec![]).await.unwrap();
        if !bridge.state.mappings[&a.key()].active {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(!bridge.state.mappings[&a.key()].active);
    assert!(
        bridge
            .state
            .outbox
            .iter()
            .any(|item| item.text == "Final after exit")
    );
}

#[tokio::test]
async fn background_export_failure_still_announces_a_blocked_session() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    std::fs::write(&config.opencode_bin, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut bridge = Bridge::open(config).unwrap();
    let mut a = agent();
    bridge.state.mappings.insert(
        a.key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            output_initialised: true,
            active: true,
            agent_record: Some(a.clone()),
            last_agent_status: "idle".into(),
            last_state_change_seq: 1,
            ..Mapping::default()
        },
    );
    a.agent_status = "blocked".into();
    a.state_change_seq = 2;
    for _ in 0..100 {
        bridge.poll_sessions(vec![a.clone()]).await.unwrap();
        if bridge
            .state
            .outbox
            .iter()
            .any(|item| item.text.contains("Agent needs input in the terminal"))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        bridge
            .state
            .outbox
            .iter()
            .any(|item| item.text.contains("Agent needs input in the terminal"))
    );
}

#[tokio::test]
async fn background_task_export_failure_does_not_hide_working_status() {
    let root = tempfile::tempdir().unwrap();
    let config = editable_config(root.path());
    std::fs::write(&config.opencode_bin, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut bridge = Bridge::open(config).unwrap();
    let mut a = agent();
    bridge.state.mappings.insert(
        a.key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            output_initialised: true,
            active: true,
            agent_record: Some(a.clone()),
            last_agent_status: "idle".into(),
            last_state_change_seq: 1,
            ..Mapping::default()
        },
    );
    a.agent_status = "working".into();
    a.state_change_seq = 2;
    for _ in 0..100 {
        bridge.poll_sessions(vec![a.clone()]).await.unwrap();
        if bridge.state.mappings[&a.key()].last_agent_status == "working" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(bridge.state.mappings[&a.key()].last_agent_status, "working");
    assert!(
        bridge
            .state
            .outbox
            .iter()
            .any(|item| item.text == "> working")
    );
}

#[tokio::test]
async fn background_reads_discard_a_snapshot_from_an_older_agent_state() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let source = root.path().join("export.json");
    let started = root.path().join("started");
    let release = root.path().join("release");
    let data = |id: &str, text: &str| {
        json!({"messages":[{"info":{"id":id,"role":"assistant","time":{"completed":1},"finish":"stop"},"parts":[{"type":"text","text":text}]}]}).to_string()
    };
    std::fs::write(&source, data("stale", "Stale final")).unwrap();
    std::fs::write(&config.opencode_bin, format!("#!/bin/sh\ndata=$(cat {source:?})\ntouch {started:?}\nwhile [ ! -e {release:?} ]; do sleep 0.01; done\nprintf '%s' \"$data\"\n")).unwrap();
    std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut bridge = Bridge::open(config).unwrap();
    let mut a = agent();
    bridge.state.mappings.insert(
        a.key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            output_initialised: true,
            active: true,
            agent_record: Some(a.clone()),
            last_agent_status: "idle".into(),
            last_state_change_seq: 1,
            ..Mapping::default()
        },
    );
    bridge.poll_sessions(vec![a.clone()]).await.unwrap();
    for _ in 0..100 {
        if started.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(started.exists());
    a.state_change_seq = 2;
    std::fs::write(&source, data("current", "Current final")).unwrap();
    std::fs::write(release, "").unwrap();
    for _ in 0..100 {
        bridge.poll_sessions(vec![a.clone()]).await.unwrap();
        if bridge
            .state
            .outbox
            .iter()
            .any(|item| item.text == "Current final")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        bridge
            .state
            .outbox
            .iter()
            .any(|item| item.text == "Current final")
    );
    assert!(
        !bridge
            .state
            .outbox
            .iter()
            .any(|item| item.text == "Stale final")
    );
}

#[tokio::test]
async fn background_reads_limit_parallel_exports_and_ignore_results_after_desk_selection() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let starts = root.path().join("starts");
    let release = root.path().join("release");
    let data = json!({"messages":[{"info":{"id":"new-final","role":"assistant","time":{"completed":1},"finish":"stop"},"parts":[{"type":"text","text":"Must stay muted"}]}]});
    std::fs::write(&config.opencode_bin, format!("#!/bin/sh\nprintf '%s\\n' \"$3\" >> {starts:?}\nwhile [ ! -e {release:?} ]; do sleep 0.01; done\nprintf '%s' '{data}'\n")).unwrap();
    std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut bridge = Bridge::open(config.clone()).unwrap();
    let agents: Vec<_> = (0..6)
        .map(|index| {
            let mut a = agent();
            a.agent_session.as_mut().unwrap().value = format!("ses_{index}");
            a.pane_id = format!("w1:p{index}");
            bridge.state.mappings.insert(
                a.key(),
                Mapping {
                    group_id_hex: Some(format!("{index:032x}")),
                    output_initialised: true,
                    active: true,
                    agent_record: Some(a.clone()),
                    last_agent_status: "idle".into(),
                    last_state_change_seq: 1,
                    ..Mapping::default()
                },
            );
            a
        })
        .collect();
    bridge.poll_sessions(agents.clone()).await.unwrap();
    for _ in 0..100 {
        if std::fs::read_to_string(&starts)
            .unwrap_or_default()
            .lines()
            .count()
            == 4
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(std::fs::read_to_string(&starts).unwrap().lines().count(), 4);
    assert!(forwarding_command(&config, "desk").await.status.success());
    std::fs::write(release, "").unwrap();
    for _ in 0..100 {
        bridge.poll_sessions(agents.clone()).await.unwrap();
        if bridge
            .state
            .mappings
            .values()
            .all(|m| !m.forwarding_baseline_pending)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(!bridge.state.forwarding_enabled);
    assert!(
        bridge
            .state
            .mappings
            .values()
            .all(|m| !m.forwarding_baseline_pending)
    );
    assert!(bridge.state.outbox.is_empty());
}

fn editable_config(dir: &std::path::Path) -> BridgeConfig {
    serde_json::from_value(json!({
        "phone_npub":phone_npub(), "herdr_socket":dir.join("herdr.sock"),
        "wn_agent_socket":dir.join("wn.sock"), "account_id_hex":"aa".repeat(32),
        "state_path":dir.join("state.json"), "opencode_bin":dir.join("opencode"),
        "codex_home":dir.join("codex"), "claude_home":dir.join("claude"),
        "edit_responses":true, "auto_forwarding":false
    }))
    .unwrap()
}

async fn forwarding_command(config: &BridgeConfig, mode: &str) -> std::process::Output {
    let path = config.state_path.with_file_name("test-config.toml");
    std::fs::write(&path,format!("phone_npub = {:?}\naccount_id_hex = {:?}\nherdr_socket = {:?}\nwn_agent_socket = {:?}\nstate_path = {:?}\nopencode_bin = {:?}\ncodex_home = {:?}\nclaude_home = {:?}\nedit_responses = {}\n",config.phone_npub,config.account_id_hex,config.herdr_socket,config.wn_agent_socket,config.state_path,config.opencode_bin,config.codex_home,config.claude_home,config.edit_responses)).unwrap();
    tokio::process::Command::new(env!("CARGO_BIN_EXE_herdr-white-noise-bridge"))
        .args(["--config", path.to_str().unwrap(), "forwarding", mode])
        .output()
        .await
        .unwrap()
}

fn decision_export(path: &std::path::Path, id: &str, question: &str) {
    let data = json!({"messages":[
        {"info":{"id":"user-decision","role":"user"},"parts":[]},
        {"info":{"id":"assistant-decision","parentID":"user-decision","role":"assistant"},"parts":[
            {"type":"text","text":"PRIVATE REASONING"},
            {"type":"tool","id":id,"tool":"question","state":{"status":"running","input":{"questions":[{
                "header":"Next step","question":question,"options":[
                    {"label":"Proceed","description":"Continue with the plan"},
                    {"label":"Review","description":"Review the alternatives"}
                ]
            }]}}}
        ]}
    ]});
    std::fs::write(path, format!("#!/bin/sh\nprintf '%s' '{}'\n", data)).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn decision_terminal(question: &str) -> String {
    format!(
        "PRIVATE TOOL OUTPUT\n{question}\n1. Proceed\n   Continue with the plan\n2. Review\n   Review the alternatives\n3. Type your own answer\n↑↓ select enter submit esc dismiss\n"
    )
}

async fn decision_herdr(
    path: &std::path::Path,
    observed: Arc<Mutex<Agent>>,
    visible: Arc<Mutex<String>>,
    calls: Arc<Mutex<Vec<Value>>>,
) -> tokio::task::JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(stream);
            let request = read_line(&mut reader).await.unwrap();
            calls.lock().unwrap().push(request.clone());
            let mut response = match request["method"].as_str().unwrap() {
                "agent.get" => json!({"result":{"agent":*observed.lock().unwrap()}}),
                "agent.read" => {
                    json!({"result":{"read":{"text":*visible.lock().unwrap(),"truncated":false}}})
                }
                "agent.send_keys" => {
                    if visible.lock().unwrap().starts_with("DROP_SELECTION_ACK") {
                        continue;
                    }
                    assert_eq!(
                        request["params"]["keys"].as_array().unwrap().len(),
                        1,
                        "Never append Enter after a numbered single-choice shortcut"
                    );
                    let mut current = observed.lock().unwrap();
                    current.agent_status = "working".into();
                    current.state_change_seq += 1;
                    json!({"result":{"type":"ok"}})
                }
                _ => panic!("Decision replies must not use agent.prompt"),
            };
            response["id"] = request["id"].clone();
            reader
                .get_mut()
                .write_all((response.to_string() + "\n").as_bytes())
                .await
                .unwrap();
        }
    })
}

#[tokio::test]
async fn a_phone_choice_answers_the_blocked_question_instead_of_becoming_a_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    decision_export(&config.opencode_bin, "question-1", "Which next step?");
    let mut blocked = agent();
    blocked.agent_status = "blocked".into();
    blocked.state_change_seq = 2;
    let observed = Arc::new(Mutex::new(blocked.clone()));
    let visible = Arc::new(Mutex::new(decision_terminal("Which next step?")));
    let calls = Arc::new(Mutex::new(vec![]));
    let herdr = decision_herdr(
        &config.herdr_socket,
        observed.clone(),
        visible,
        calls.clone(),
    )
    .await;
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            pending_prompt: true,
            agent_record: Some(agent()),
            last_agent_status: "working".into(),
            last_state_change_seq: 1,
            ..Mapping::default()
        },
    );
    bridge.reconcile(vec![blocked]).await.unwrap();
    bridge.flush().await.unwrap();
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| { call["text"] == "> blocked Waiting for your decision." })
    );
    let mut answer = inbound(HEX, "choice-1");
    answer["message"]["text"] = json!("1");
    bridge.inbound(&answer).await.unwrap();
    bridge.flush().await.unwrap();
    let selected: Vec<_> = calls
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r["method"] == "agent.send_keys")
        .cloned()
        .collect();
    assert_eq!(
        selected.len(),
        1,
        "A blocked question is waiting for an answer, not a new prompt"
    );
    assert_eq!(selected[0]["params"]["keys"], json!(["1"]));
    assert!(
        !calls.lock().unwrap().iter().any(|r| r["text"]
            .as_str()
            .is_some_and(|text| text.contains("PRIVATE TOOL OUTPUT")
                || text.contains("PRIVATE REASONING")
                || text.contains("terminal prompt")))
    );
    let mut restarted = Bridge::open(config).unwrap();
    let current = observed.lock().unwrap().clone();
    restarted.reconcile(vec![current]).await.unwrap();
    restarted.inbound(&answer).await.unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["method"] == "agent.send_keys")
            .count(),
        1
    );
    mdk.abort();
    herdr.abort();
}

#[tokio::test]
async fn desk_mode_discards_updates_and_away_does_not_replay_them() {
    for edits in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config(dir.path());
        config.edit_responses = edits;
        export_script(&config.opencode_bin, "Old reply", "old");
        let calls = Arc::new(Mutex::new(vec![]));
        let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
        let mut bridge = Bridge::open(config.clone()).unwrap();
        bridge.reconcile(vec![agent()]).await.unwrap();
        bridge.flush().await.unwrap();
        calls.lock().unwrap().clear();
        let result = forwarding_command(&config, "desk").await;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        export_script(&config.opencode_bin, "Muted reply", "muted");
        let mut done = agent();
        done.state_change_seq = 2;
        bridge.reconcile(vec![done.clone()]).await.unwrap();
        bridge.flush().await.unwrap();
        assert!(calls.lock().unwrap().is_empty());
        assert!(
            bridge.state.mappings[&agent().key()]
                .final_ids
                .contains(&"muted".into())
        );
        assert!(bridge.state.outbox.is_empty());
        assert!(forwarding_command(&config, "away").await.status.success());
        let mut bridge = Bridge::open(config.clone()).unwrap();
        bridge.reconcile(vec![done]).await.unwrap();
        bridge.flush().await.unwrap();
        assert!(calls.lock().unwrap().is_empty(), "No backlog on resume");
        let mut working = agent();
        working.agent_status = "working".into();
        working.state_change_seq = 3;
        bridge.reconcile(vec![working]).await.unwrap();
        bridge.flush().await.unwrap();
        export_script(&config.opencode_bin, "Future reply", "future");
        let mut done = agent();
        done.state_change_seq = 4;
        bridge.reconcile(vec![done]).await.unwrap();
        bridge.flush().await.unwrap();
        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .any(|request| request["text"] == "Future reply")
        );
        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|request| request["text"] == "Muted reply")
        );
        mdk.abort();
    }
}

#[tokio::test]
async fn only_a_new_authorised_phone_message_wakes_forwarding_for_all_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    export_script(&config.opencode_bin, "Old reply", "old");
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    let mut other = agent();
    other.agent_session.as_mut().unwrap().value = "ses_2".into();
    other.pane_id = "w1:p2".into();
    other.terminal_id = "term2".into();
    for (observed, group) in [(agent(), "bb".repeat(16)), (other.clone(), "dd".repeat(16))] {
        bridge.state.mappings.insert(
            observed.key(),
            Mapping {
                group_id_hex: Some(group),
                active: true,
                agent_record: Some(observed),
                last_agent_status: "idle".into(),
                last_state_change_seq: 1,
                final_ids: vec!["old".into()],
                output_initialised: true,
                ..Mapping::default()
            },
        );
    }
    bridge
        .state
        .mappings
        .get_mut(&agent().key())
        .unwrap()
        .seen_messages
        .push("already".into());
    bridge.save().unwrap();
    assert!(forwarding_command(&config, "desk").await.status.success());
    bridge
        .reconcile(vec![agent(), other.clone()])
        .await
        .unwrap();
    bridge.flush().await.unwrap();
    bridge
        .inbound(&inbound(&"ff".repeat(32), "foreign"))
        .await
        .unwrap();
    let mut orphan = inbound(HEX, "orphan");
    orphan["group_id_hex"] = json!("ee".repeat(16));
    bridge.inbound(&orphan).await.unwrap();
    bridge.inbound(&inbound(HEX, "already")).await.unwrap();
    assert_eq!(
        serde_json::to_value(&bridge.state).unwrap()["forwarding_enabled"],
        false
    );
    let mut wake = inbound(HEX, "wake");
    wake["message"]["text"] = json!("/status");
    bridge.inbound(&wake).await.unwrap();
    bridge.flush().await.unwrap();
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| { call["text"] == "> idle HERDR session: connected." })
    );
    assert_eq!(
        serde_json::to_value(&bridge.state).unwrap()["forwarding_enabled"],
        true
    );
    let mut restarted = Bridge::open(config.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&restarted.state).unwrap()["forwarding_enabled"],
        true
    );
    other.agent_status = "working".into();
    other.state_change_seq = 2;
    restarted.reconcile(vec![agent(), other]).await.unwrap();
    restarted.flush().await.unwrap();
    assert!(calls.lock().unwrap().iter().any(
        |request| request["group_id_hex"] == "dd".repeat(16) && request["text"] == "> working"
    ));
    assert!(forwarding_command(&config, "desk").await.status.success());
    restarted.reconcile(vec![agent()]).await.unwrap();
    restarted.inbound(&wake).await.unwrap();
    assert_eq!(
        serde_json::to_value(&restarted.state).unwrap()["forwarding_enabled"],
        false,
        "History replay must not undo a later Desk choice"
    );
    mdk.abort();
}

#[tokio::test]
async fn a_replaced_question_does_not_accept_an_old_unqualified_answer() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    decision_export(&config.opencode_bin, "question-a", "First question?");
    let mut blocked = agent();
    blocked.agent_status = "blocked".into();
    blocked.state_change_seq = 2;
    let observed = Arc::new(Mutex::new(blocked.clone()));
    let visible = Arc::new(Mutex::new(decision_terminal("First question?")));
    let calls = Arc::new(Mutex::new(vec![]));
    let herdr = decision_herdr(
        &config.herdr_socket,
        observed,
        visible.clone(),
        calls.clone(),
    )
    .await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            pending_prompt: true,
            agent_record: Some(agent()),
            ..Mapping::default()
        },
    );
    bridge.reconcile(vec![blocked]).await.unwrap();
    decision_export(&config.opencode_bin, "question-b", "Second question?");
    *visible.lock().unwrap() = decision_terminal("Second question?");
    let mut answer = inbound(HEX, "old-answer");
    answer["message"]["text"] = json!("1");
    bridge.inbound(&answer).await.unwrap();
    answer["message"]["message_id_hex"] = json!("still-old-answer");
    bridge.inbound(&answer).await.unwrap();
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|request| request["method"] == "agent.send_keys")
    );
    let decision = bridge.state.mappings[&agent().key()]
        .decision
        .as_ref()
        .unwrap();
    assert!(decision.requires_token);
    answer["message"]["message_id_hex"] = json!("current-answer");
    answer["message"]["text"] = json!(format!("{} 2", decision.token()));
    bridge.inbound(&answer).await.unwrap();
    let selected: Vec<_> = calls
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r["method"] == "agent.send_keys")
        .cloned()
        .collect();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0]["params"]["keys"], json!(["2"]));
    herdr.abort();
}

#[tokio::test]
async fn permission_or_changed_terminal_ui_cannot_receive_a_question_shortcut() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    decision_export(&config.opencode_bin, "question-1", "Which next step?");
    let mut blocked = agent();
    blocked.agent_status = "blocked".into();
    blocked.state_change_seq = 2;
    let observed = Arc::new(Mutex::new(blocked.clone()));
    let visible = Arc::new(Mutex::new(
        "Allow writing files? enter confirm esc cancel".into(),
    ));
    let calls = Arc::new(Mutex::new(vec![]));
    let herdr = decision_herdr(&config.herdr_socket, observed, visible, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            pending_prompt: true,
            agent_record: Some(agent()),
            ..Mapping::default()
        },
    );
    bridge.reconcile(vec![blocked]).await.unwrap();
    let mut answer = inbound(HEX, "invalid-ui");
    answer["message"]["text"] = json!("1");
    bridge.inbound(&answer).await.unwrap();
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|request| request["method"] == "agent.send_keys")
    );
    assert!(
        !bridge.state.mappings[&agent().key()]
            .decision
            .as_ref()
            .unwrap()
            .submitted
    );
    assert!(
        bridge
            .state
            .outbox
            .iter()
            .any(|message| message.text.contains("No answer was sent"))
    );
    herdr.abort();
}

#[tokio::test]
async fn an_uncertain_decision_shortcut_is_not_sent_again_on_retry_or_restart() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    decision_export(&config.opencode_bin, "question-1", "Which next step?");
    let mut blocked = agent();
    blocked.agent_status = "blocked".into();
    blocked.state_change_seq = 2;
    let observed = Arc::new(Mutex::new(blocked.clone()));
    let visible = Arc::new(Mutex::new(format!(
        "DROP_SELECTION_ACK\n{}",
        decision_terminal("Which next step?")
    )));
    let calls = Arc::new(Mutex::new(vec![]));
    let herdr = decision_herdr(&config.herdr_socket, observed, visible, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            pending_prompt: true,
            agent_record: Some(agent()),
            ..Mapping::default()
        },
    );
    bridge.reconcile(vec![blocked.clone()]).await.unwrap();
    let mut answer = inbound(HEX, "uncertain-answer");
    answer["message"]["text"] = json!("1");
    bridge.inbound(&answer).await.unwrap();
    assert!(
        bridge.state.mappings[&agent().key()]
            .decision
            .as_ref()
            .unwrap()
            .submitted
    );
    let mut restarted = Bridge::open(config).unwrap();
    restarted.reconcile(vec![blocked]).await.unwrap();
    restarted.inbound(&answer).await.unwrap();
    answer["message"]["message_id_hex"] = json!("another-answer");
    restarted.inbound(&answer).await.unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["method"] == "agent.send_keys")
            .count(),
        1
    );
    herdr.abort();
}

#[tokio::test]
async fn a_whitespace_or_uppercase_coded_answer_cannot_become_a_new_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    decision_export(&config.opencode_bin, "question-1", "Which next step?");
    let mut blocked = agent();
    blocked.agent_status = "blocked".into();
    blocked.state_change_seq = 2;
    let observed = Arc::new(Mutex::new(blocked.clone()));
    let visible = Arc::new(Mutex::new(decision_terminal("Which next step?")));
    let calls = Arc::new(Mutex::new(vec![]));
    let herdr = decision_herdr(
        &config.herdr_socket,
        observed.clone(),
        visible,
        calls.clone(),
    )
    .await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            agent_record: Some(agent()),
            ..Mapping::default()
        },
    );
    bridge.reconcile(vec![blocked]).await.unwrap();
    let token = bridge.state.mappings[&agent().key()]
        .decision
        .as_ref()
        .unwrap()
        .token();
    let mut idle = agent();
    idle.state_change_seq = 3;
    *observed.lock().unwrap() = idle.clone();
    export_script(&config.opencode_bin, "Finished", "finished");
    bridge.reconcile(vec![idle]).await.unwrap();
    let mut answer = inbound(HEX, "stale-coded");
    answer["message"]["text"] = json!(format!(" {} 1", token.to_uppercase()));
    bridge.inbound(&answer).await.unwrap();
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|request| request["method"] == "agent.prompt")
    );
    assert!(
        bridge
            .state
            .outbox
            .iter()
            .any(|message| message.text.contains("no longer waiting"))
    );
    herdr.abort();
}

#[tokio::test]
async fn forwarding_controls_work_while_the_worker_holds_its_state_lock() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let bridge = Bridge::open(config.clone()).unwrap();
    let _worker = herdr_white_noise_bridge::state::lock(&config.state_path).unwrap();
    let original = std::fs::read(&config.state_path).unwrap();
    let desk = forwarding_command(&config, "desk").await;
    assert!(
        desk.status.success(),
        "{}",
        String::from_utf8_lossy(&desk.stderr)
    );
    assert!(String::from_utf8_lossy(&desk.stdout).contains("At desk"));
    assert!(forwarding_command(&config, "status").await.status.success());
    let away = forwarding_command(&config, "toggle").await;
    assert!(away.status.success());
    assert!(String::from_utf8_lossy(&away.stdout).contains("Away"));
    let control = config.state_path.with_extension("forwarding.json");
    assert_eq!(
        std::fs::metadata(&control).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let current = std::fs::read(&control).unwrap();
    assert!(forwarding_command(&config, "away").await.status.success());
    assert_eq!(
        std::fs::read(&control).unwrap(),
        current,
        "Setting the current mode must not create another reset"
    );
    assert_eq!(
        std::fs::read(&config.state_path).unwrap(),
        original,
        "Only the worker writes routing state"
    );
    assert_eq!(bridge.state.mappings.len(), 0);
}

#[tokio::test]
async fn a_deferred_phone_wake_does_not_undo_a_later_desk_selection() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            agent_record: Some(agent()),
            ..Mapping::default()
        },
    );
    assert!(forwarding_command(&config, "desk").await.status.success());
    let mut wake = inbound(HEX, "deferred");
    wake["message"]["text"] = json!("/status");
    assert!(bridge.inbound(&wake).await.is_err());
    assert!(
        String::from_utf8_lossy(&forwarding_command(&config, "status").await.stdout)
            .contains("Away")
    );
    assert!(
        bridge.state.mappings[&agent().key()]
            .seen_messages
            .is_empty(),
        "The prompt must still be recoverable"
    );
    assert!(forwarding_command(&config, "desk").await.status.success());
    assert!(bridge.inbound(&wake).await.is_err());
    assert!(
        String::from_utf8_lossy(&forwarding_command(&config, "status").await.stdout)
            .contains("At desk"),
        "Retrying one received message must not apply its wake twice"
    );
}

#[tokio::test]
async fn selecting_desk_during_a_transcript_read_prevents_a_new_invitation() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let ready = dir.path().join("ready");
    let release = dir.path().join("release");
    std::fs::write(&config.opencode_bin,format!("#!/bin/sh\ntouch '{}'\nwhile [ ! -f '{}' ]; do sleep 0.01; done\nprintf '%s' '{{\"messages\":[]}}'\n",ready.display(),release.display())).unwrap();
    std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    let reconcile = tokio::spawn(async move {
        bridge.reconcile(vec![agent()]).await.unwrap();
        bridge
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !ready.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(forwarding_command(&config, "desk").await.status.success());
    std::fs::write(release, "").unwrap();
    let bridge = reconcile.await.unwrap();
    assert!(
        calls.lock().unwrap().is_empty(),
        "Do not submit group_create after the Desk control was written"
    );
    assert!(bridge.state.mappings[&agent().key()].group_id_hex.is_none());
    mdk.abort();
}

#[tokio::test]
async fn a_late_record_from_a_muted_turn_is_not_forwarded_after_resume() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    let write = |final_reply: Option<(&str, &str)>, turn: &str| {
        let mut messages = vec![json!({"info":{"id":turn,"role":"user"},"parts":[]})];
        if let Some((id, text)) = final_reply {
            messages.push(json!({"info":{"id":id,"parentID":turn,"role":"assistant","time":{"completed":1},"finish":"stop"},"parts":[{"type":"text","text":text}]}));
        }
        std::fs::write(
            &config.opencode_bin,
            format!(
                "#!/bin/sh\nprintf '%s' '{}'\n",
                json!({"messages":messages})
            ),
        )
        .unwrap();
        std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700))
            .unwrap();
    };
    write(None, "muted-user-turn");
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            agent_record: Some(agent()),
            last_agent_status: "idle".into(),
            last_state_change_seq: 1,
            ..Mapping::default()
        },
    );
    assert!(forwarding_command(&config, "desk").await.status.success());
    bridge.reconcile(vec![agent()]).await.unwrap();
    assert!(forwarding_command(&config, "away").await.status.success());
    bridge.reconcile(vec![agent()]).await.unwrap();
    write(Some(("late", "Late muted reply")), "muted-user-turn");
    let mut idle = agent();
    idle.state_change_seq = 2;
    bridge.reconcile(vec![idle.clone()]).await.unwrap();
    bridge.flush().await.unwrap();
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|request| request["text"] == "Late muted reply")
    );
    write(Some(("new", "New away reply")), "new-user-turn");
    idle.state_change_seq = 3;
    bridge.reconcile(vec![idle]).await.unwrap();
    bridge.flush().await.unwrap();
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|request| request["text"] == "New away reply")
    );
    mdk.abort();
}

#[tokio::test]
async fn global_phone_wake_checkpoints_other_sessions_before_returning() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    export_script(&config.opencode_bin, "Old reply", "old");
    let mut bridge = Bridge::open(config.clone()).unwrap();
    let mut other = agent();
    other.agent_session.as_mut().unwrap().value = "ses_other".into();
    for (observed, group) in [(agent(), "bb".repeat(16)), (other.clone(), "dd".repeat(16))] {
        bridge.state.mappings.insert(
            observed.key(),
            Mapping {
                group_id_hex: Some(group),
                active: true,
                agent_record: Some(observed),
                last_agent_status: "idle".into(),
                last_state_change_seq: 1,
                final_ids: vec!["old".into()],
                ..Mapping::default()
            },
        );
    }
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    assert!(forwarding_command(&config, "desk").await.status.success());
    bridge
        .reconcile(vec![agent(), other.clone()])
        .await
        .unwrap();
    let mut wake = inbound(HEX, "wake");
    wake["message"]["text"] = json!("/status");
    bridge.inbound(&wake).await.unwrap();
    export_script(
        &config.opencode_bin,
        "Reply completed after wake",
        "after-wake",
    );
    other.state_change_seq = 2;
    bridge.reconcile(vec![agent(), other]).await.unwrap();
    bridge.flush().await.unwrap();
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|request| request["group_id_hex"] == "dd".repeat(16)
                && request["text"] == "Reply completed after wake")
    );
    mdk.abort();
}

#[tokio::test]
async fn a_database_upgrade_recovers_phone_input_once_without_replaying_muted_history() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    let path = root.path().join("opencode.db");
    config.opencode_db = Some(path.clone());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TABLE session(id TEXT); INSERT INTO session VALUES('ses_1');
        CREATE TABLE event(id TEXT,aggregate_id TEXT,seq INTEGER,type TEXT,data TEXT);
        INSERT INTO event VALUES('event-0','ses_1',0,'session.created.1','{}');
        CREATE TABLE message(id TEXT,session_id TEXT,time_created INTEGER,data TEXT);
        CREATE TABLE part(id TEXT,session_id TEXT,message_id TEXT,data TEXT);",
    )
    .unwrap();
    let old = json!({"info":{"id":"ses_1"},"messages":[{"type":"user","id":"muted-user","text":"old prompt"},{"type":"assistant","id":"muted-reply","finish":"stop","time":{"completed":1},"content":[{"type":"text","text":"Never replay this reply"}]}]});
    let calls_path = root.path().join("cli-calls");
    let failing = root.path().join("export-fails");
    std::fs::write(&config.opencode_bin, format!("#!/bin/sh\nif [ \"$1\" = --pure ] || [ -f {failing:?} ]; then exit 2; fi\nprintf x >> {calls_path:?}\nprintf '%s' '{old}'\n")).unwrap();
    std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer(&config.herdr_socket, false, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            output_initialised: true,
            active: true,
            agent_record: Some(agent()),
            last_agent_status: "idle".into(),
            last_state_change_seq: 1,
            ..Mapping::default()
        },
    );
    bridge.reconcile(vec![agent()]).await.unwrap();
    assert!(
        !calls_path.exists(),
        "Compatible v1 history must stay on SQLite"
    );
    db.execute("DELETE FROM event", []).unwrap();
    std::fs::write(&failing, b"fixture").unwrap();
    assert!(forwarding_command(&config, "desk").await.status.success());
    bridge.poll_sessions(vec![agent()]).await.unwrap();
    let event = inbound(HEX, "phone-after-upgrade");
    assert!(bridge.inbound(&event).await.is_err());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(bridge.poll_ready_sessions().await.is_err());
    assert!(
        !bridge.state.mappings[&agent().key()]
            .seen_messages
            .contains(&"phone-after-upgrade".into())
    );
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|request| request["method"] == "agent.prompt")
    );
    assert!(bridge.state.forwarding_resume_pending);
    std::fs::remove_file(&failing).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let _ = bridge.poll_ready_sessions().await;
            if bridge.inbound(&event).await.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(bridge.state.forwarding_enabled && !bridge.state.forwarding_resume_pending);
    assert!(calls_path.exists());
    assert!(
        bridge.state.mappings[&agent().key()]
            .final_ids
            .contains(&"muted-reply".into())
    );
    assert!(
        !bridge
            .state
            .outbox
            .iter()
            .any(|item| item.text.contains("Never replay"))
    );
    bridge.inbound(&event).await.unwrap();
    drop(bridge);
    let mut reopened = Bridge::open(config).unwrap();
    reopened.inbound(&event).await.unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request["method"] == "agent.prompt")
            .count(),
        1
    );
    mdk.abort();
    herdr.abort();
}

#[tokio::test]
async fn a_superseded_muted_turn_stays_muted_while_the_current_turn_can_reply() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    let user = |id: &str| json!({"info":{"id":id,"role":"user"},"parts":[]});
    let reply = |id: &str, parent: &str, text: &str| json!({"info":{"id":id,"parentID":parent,"role":"assistant","time":{"completed":1},"finish":"stop"},"parts":[{"type":"text","text":text}]});
    let write = |messages: Vec<Value>| {
        std::fs::write(
            &config.opencode_bin,
            format!(
                "#!/bin/sh\nprintf '%s' '{}'\n",
                json!({"messages":messages})
            ),
        )
        .unwrap();
        std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700))
            .unwrap();
    };
    write(vec![user("turn-a"), user("turn-b")]);
    let mut bridge = Bridge::open(config.clone()).unwrap();
    let mut working = agent();
    working.agent_status = "working".into();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            agent_record: Some(working.clone()),
            last_agent_status: "working".into(),
            ..Mapping::default()
        },
    );
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    assert!(forwarding_command(&config, "desk").await.status.success());
    bridge.reconcile(vec![working.clone()]).await.unwrap();
    assert!(forwarding_command(&config, "away").await.status.success());
    bridge.reconcile(vec![working]).await.unwrap();
    bridge.flush().await.unwrap();
    write(vec![
        user("turn-a"),
        reply("late-a", "turn-a", "Late muted A"),
        user("turn-b"),
    ]);
    let mut idle = agent();
    idle.state_change_seq = 2;
    bridge.reconcile(vec![idle.clone()]).await.unwrap();
    bridge.flush().await.unwrap();
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|request| request["text"] == "Late muted A")
    );
    write(vec![
        user("turn-a"),
        reply("late-a", "turn-a", "Late muted A"),
        user("turn-b"),
        reply("final-b", "turn-b", "Current away B"),
    ]);
    idle.state_change_seq = 3;
    bridge.reconcile(vec![idle]).await.unwrap();
    bridge.flush().await.unwrap();
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|request| request["text"] == "Current away B")
    );
    mdk.abort();
}

#[tokio::test]
async fn legacy_state_reports_away_requested_until_the_worker_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let mut legacy = BridgeState::default();
    legacy.mappings.insert(agent().key(), Mapping::default());
    legacy.save(&config.state_path).unwrap();
    assert!(forwarding_command(&config, "desk").await.status.success());
    assert!(forwarding_command(&config, "away").await.status.success());
    let status = forwarding_command(&config, "status").await;
    assert!(String::from_utf8_lossy(&status.stdout).contains("Away requested"));
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_herdr-white-noise-bridge"))
        .args([
            "--config",
            config
                .state_path
                .with_file_name("test-config.toml")
                .to_str()
                .unwrap(),
            "status",
        ])
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(row["forwarding_enabled"], false);
    assert_eq!(row["forwarding_resume_pending"], true);
}

#[tokio::test]
async fn global_resume_reads_independent_session_checkpoints_concurrently() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let ready1 = dir.path().join("ses_1");
    let ready2 = dir.path().join("ses_2");
    let release = dir.path().join("release");
    std::fs::write(&config.opencode_bin,format!("#!/bin/sh\ntouch '{}/'$3\nwhile [ ! -f '{}' ]; do sleep 0.01; done\nprintf '%s' '{{\"messages\":[]}}'\n",dir.path().display(),release.display())).unwrap();
    std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut bridge = Bridge::open(config.clone()).unwrap();
    let mut other = agent();
    other.agent_session.as_mut().unwrap().value = "ses_2".into();
    for observed in [agent(), other.clone()] {
        bridge.state.mappings.insert(
            observed.key(),
            Mapping {
                group_id_hex: Some("bb".repeat(16)),
                active: true,
                output_initialised: true,
                agent_record: Some(observed),
                last_agent_status: "idle".into(),
                last_state_change_seq: 1,
                ..Mapping::default()
            },
        );
    }
    assert!(forwarding_command(&config, "desk").await.status.success());
    assert!(forwarding_command(&config, "away").await.status.success());
    let reconcile = tokio::spawn(async move {
        bridge.reconcile(vec![agent(), other]).await.unwrap();
        bridge
    });
    let concurrent = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !ready1.exists() || !ready2.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    std::fs::write(release, "").unwrap();
    let bridge = reconcile.await.unwrap();
    assert!(
        concurrent.is_ok(),
        "One slow export must not serialize every session checkpoint"
    );
    assert!(bridge.state.forwarding_enabled);
}

#[tokio::test]
async fn progress_is_edited_but_the_final_is_a_separate_message_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    export_script(&config.opencode_bin, "old answer", "old");
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer(&config.herdr_socket, false, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    bridge.flush().await.unwrap();
    calls.lock().unwrap().clear();
    bridge.inbound(&inbound(HEX, "first-prompt")).await.unwrap();
    bridge.flush().await.unwrap();
    let export = json!({"messages":[
        {"info":{"id":"user","role":"user"},"parts":[]},
        {"info":{"id":"progress","role":"assistant"},"parts":[
            {"type":"text","text":"Private reasoning must stay local"},
            {"type":"tool","id":"todo-1","tool":"todowrite","state":{"status":"completed","input":{"todos":[
                {"content":"Inspect code","status":"completed","priority":"high"},
                {"content":"Run checks","status":"in_progress","priority":"high"}
            ]}}}
        ]}
    ]});
    std::fs::write(
        &config.opencode_bin,
        format!("#!/bin/sh\nprintf '%s' '{}'\n", export),
    )
    .unwrap();
    let mut working = agent();
    working.agent_status = "working".into();
    working.state_change_seq = 2;
    bridge.reconcile(vec![working]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    bridge.flush().await.unwrap();
    drop(bridge);
    let mut bridge = Bridge::open(config.clone()).unwrap();
    export_script(&config.opencode_bin, "The final answer", "final");
    let mut done = agent();
    done.agent_status = "done".into();
    done.state_change_seq = 3;
    bridge.reconcile(vec![done.clone()]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    bridge.flush().await.unwrap();
    bridge.reconcile(vec![done]).await.unwrap();
    bridge.flush().await.unwrap();
    let requests = calls.lock().unwrap();
    let sends: Vec<_> = requests
        .iter()
        .filter(|r| r["type"] == "send_final")
        .collect();
    assert_eq!(
        sends.len(),
        2,
        "One progress message and one immutable final reply"
    );
    assert_eq!(sends[0]["text"], "> working");
    assert_eq!(sends[1]["text"], "The final answer");
    assert_ne!(sends[0]["idempotency_key"], sends[1]["idempotency_key"]);
    let edits: Vec<_> = requests
        .iter()
        .filter(|r| r["type"] == "edit_message")
        .collect();
    assert_eq!(edits.len(), 2);
    assert!(
        edits[0]["text"]
            .as_str()
            .unwrap()
            .contains("✓ Inspect code")
    );
    assert!(edits[0]["text"].as_str().unwrap().contains("▶ Run checks"));
    assert!(
        edits[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("> working\n\n✓ Inspect code")
    );
    assert!(edits[1]["text"].as_str().unwrap().starts_with("> done"));
    assert!(
        edits[1]["text"]
            .as_str()
            .unwrap()
            .contains("✓ Inspect code")
    );
    assert!(
        edits
            .iter()
            .all(|edit| !edit["text"].as_str().unwrap().contains("\nTasks\n"))
    );
    assert!(
        !edits
            .iter()
            .any(|request| request["text"] == "The final answer")
    );
    assert!(
        edits
            .iter()
            .all(|r| r["target_message_id_hex"] == "cc".repeat(32))
    );
    assert!(!requests.iter().any(|r| {
        r["text"]
            .as_str()
            .is_some_and(|t| t.contains("Private reasoning"))
    }));
    assert!(bridge.state.outbox.is_empty());
    mdk.abort();
    herdr.abort();
}

#[tokio::test]
async fn an_uncertain_progress_send_does_not_delay_or_duplicate_the_final() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    export_script(&config.opencode_bin, "old", "old");
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            last_agent_status: "idle".into(),
            final_ids: vec!["old".into()],
            agent_record: Some(agent()),
            ..Mapping::default()
        },
    );
    let mut working = agent();
    working.agent_status = "working".into();
    working.state_change_seq = 2;
    bridge.reconcile(vec![working]).await.unwrap();
    let listener = UnixListener::bind(&config.wn_agent_socket).unwrap();
    let first = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        read_line(&mut tokio::io::BufReader::new(stream))
            .await
            .unwrap()
    });
    assert!(bridge.flush().await.is_err());
    let original = first.await.unwrap();
    std::fs::remove_file(&config.wn_agent_socket).unwrap();
    export_script(&config.opencode_bin, "Latest final", "new");
    bridge.reconcile(vec![agent()]).await.unwrap();
    let mut restarted = Bridge::open(config.clone()).unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    restarted.flush().await.unwrap();
    restarted.flush().await.unwrap();
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "The final is independent of the uncertain progress send"
    );
    let calls = calls.lock().unwrap();
    assert_eq!(calls[0]["text"], "Latest final");
    assert_ne!(calls[0]["idempotency_key"], original["idempotency_key"]);
    assert_eq!(calls[0]["type"], "send_final");
    assert!(restarted.state.outbox.is_empty());
    mdk.abort();
}

#[tokio::test]
async fn each_new_prompt_gets_a_new_response_and_progress_is_not_repeated() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    export_script(&config.opencode_bin, "old", "old");
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer(&config.herdr_socket, false, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    bridge.flush().await.unwrap();
    calls.lock().unwrap().clear();
    bridge.inbound(&inbound(HEX, "first")).await.unwrap();
    bridge.flush().await.unwrap();
    bridge.flush().await.unwrap();
    export_script(&config.opencode_bin, "First answer", "first-final");
    let mut idle = agent();
    idle.state_change_seq = 2;
    bridge.reconcile(vec![idle.clone()]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    bridge.flush().await.unwrap();
    bridge.inbound(&inbound(HEX, "second")).await.unwrap();
    bridge.flush().await.unwrap();
    let requests = calls.lock().unwrap();
    let sends: Vec<_> = requests
        .iter()
        .filter(|r| r["type"] == "send_final")
        .collect();
    assert_eq!(sends.len(), 3);
    assert_ne!(sends[0]["idempotency_key"], sends[2]["idempotency_key"]);
    assert_eq!(sends[1]["text"], "First answer");
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["type"] == "edit_message")
            .count(),
        1
    );
    mdk.abort();
    herdr.abort();
}

#[tokio::test]
async fn long_final_replies_keep_all_text_without_waiting_for_progress_edits() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    export_script(&config.opencode_bin, "old", "old");
    let mut bridge = Bridge::open(config.clone()).unwrap();
    let key = agent().key();
    let group = "bb".repeat(16);
    bridge.state.mappings.insert(
        key.clone(),
        Mapping {
            group_id_hex: Some(group.clone()),
            final_ids: vec!["old".into()],
            agent_record: Some(agent()),
            ..Mapping::default()
        },
    );
    bridge.state.begin_response(&key, &group);
    bridge.flush().await.unwrap();
    let text = "🙂".repeat(10000);
    export_script(&config.opencode_bin, &text, "long-final");
    bridge.reconcile(vec![agent()]).await.unwrap();
    bridge.flush().await.unwrap();
    assert_eq!(
        calls.lock().unwrap().len(),
        3,
        "Both final parts precede the throttled terminal progress edit"
    );
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    bridge.flush().await.unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls[1]["type"], "send_final");
    assert_eq!(calls[2]["type"], "send_final");
    assert_eq!(calls[3]["type"], "edit_message");
    assert_eq!(calls[3]["text"], "> done");
    assert_eq!(
        format!(
            "{}{}",
            calls[1]["text"].as_str().unwrap(),
            calls[2]["text"].as_str().unwrap()
        ),
        text
    );
    mdk.abort();
}

#[tokio::test]
async fn late_final_is_resolved_before_opening_the_next_response() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    export_script(&config.opencode_bin, "old", "old");
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer(&config.herdr_socket, false, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    bridge.flush().await.unwrap();
    bridge.inbound(&inbound(HEX, "first")).await.unwrap();
    bridge.flush().await.unwrap();
    let first = bridge.state.mappings[&agent().key()]
        .response_id
        .clone()
        .unwrap();
    let mut idle = agent();
    idle.state_change_seq = 2;
    bridge.reconcile(vec![idle]).await.unwrap();
    assert!(
        bridge.state.mappings[&agent().key()].pending_prompt,
        "Keep the first response associated with its late final"
    );
    export_script(&config.opencode_bin, "First final", "first-final");
    bridge.inbound(&inbound(HEX, "second")).await.unwrap();
    let second = bridge.state.mappings[&agent().key()]
        .response_id
        .clone()
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(
        bridge
            .state
            .outbox
            .iter()
            .find(|m| m.id == first)
            .unwrap()
            .text,
        "> done"
    );
    assert!(
        bridge
            .state
            .outbox
            .iter()
            .any(|item| item.response.is_none() && item.text == "First final")
    );
    assert_eq!(
        bridge
            .state
            .outbox
            .iter()
            .find(|m| m.id == second)
            .unwrap()
            .text,
        "> working"
    );
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["method"] == "agent.prompt")
            .count(),
        2
    );
    mdk.abort();
    herdr.abort();
}

#[tokio::test]
async fn an_ended_session_closes_an_unfinished_response_before_restoration() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    export_script(&config.opencode_bin, "old", "old");
    let mut bridge = Bridge::open(config).unwrap();
    let key = agent().key();
    let group = "bb".repeat(16);
    bridge.state.mappings.insert(
        key.clone(),
        Mapping {
            group_id_hex: Some(group.clone()),
            active: true,
            agent_record: Some(agent()),
            final_ids: vec!["old".into()],
            ..Mapping::default()
        },
    );
    bridge.state.begin_response(&key, &group);
    let first = bridge.state.mappings[&key].response_id.clone().unwrap();
    bridge.reconcile(vec![]).await.unwrap();
    assert!(bridge.state.mappings[&key].response_id.is_none());
    let closed = bridge.state.outbox.iter().find(|m| m.id == first).unwrap();
    assert!(closed.response.as_ref().unwrap().finished);
    assert!(closed.text.contains("ended"));
    let mut working = agent();
    working.agent_status = "working".into();
    bridge.reconcile(vec![working]).await.unwrap();
    assert_ne!(
        bridge.state.mappings[&key].response_id.as_ref(),
        Some(&first)
    );
}

#[tokio::test]
async fn edit_cooldown_also_starts_when_a_slow_request_completes() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    let calls = Arc::new(Mutex::new(vec![]));
    let captured = calls.clone();
    let listener = UnixListener::bind(&config.wn_agent_socket).unwrap();
    let server = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(stream);
            let request = read_line(&mut reader).await.unwrap();
            captured.lock().unwrap().push(request.clone());
            tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
            let response = json!({"id":request["id"],"marmot_agent_control":"marmot.agent-control.v2","type":"final_sent","message_ids_hex":["dd".repeat(32)]});
            reader
                .get_mut()
                .write_all((response.to_string() + "\n").as_bytes())
                .await
                .unwrap();
        }
    });
    let mut bridge = Bridge::open(config).unwrap();
    let key = agent().key();
    let group = "bb".repeat(16);
    bridge.state.mappings.insert(
        key.clone(),
        Mapping {
            group_id_hex: Some(group.clone()),
            ..Mapping::default()
        },
    );
    bridge.state.begin_response(&key, &group);
    let item = bridge.state.outbox.last_mut().unwrap();
    item.response.as_mut().unwrap().target = Some("cc".repeat(32));
    item.response.as_mut().unwrap().sent_text = Some("Earlier text".into());
    bridge.flush().await.unwrap();
    bridge.state.complete_progress(&key);
    bridge.flush().await.unwrap();
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "Do not race a final edit against the late-created progress edit"
    );
    server.abort();
}

#[tokio::test]
async fn clear_prompt_releases_an_awaiting_final_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    export_script(&config.opencode_bin, "old", "old");
    let key = agent().key();
    let group = "bb".repeat(16);
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        key.clone(),
        Mapping {
            group_id_hex: Some(group.clone()),
            agent_record: Some(agent()),
            active: true,
            pending_prompt: true,
            completion_pending: true,
            last_agent_status: "idle".into(),
            last_state_change_seq: 1,
            final_ids: vec!["old".into()],
            ..Mapping::default()
        },
    );
    bridge.state.begin_response(&key, &group);
    bridge.save().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path,format!("phone_npub = {:?}\naccount_id_hex = {:?}\nherdr_socket = {:?}\nwn_agent_socket = {:?}\nstate_path = {:?}\nopencode_bin = {:?}\ncodex_home = {:?}\nclaude_home = {:?}\nedit_responses = true\n",config.phone_npub,config.account_id_hex,config.herdr_socket,config.wn_agent_socket,config.state_path,config.opencode_bin,config.codex_home,config.claude_home)).unwrap();
    drop(bridge);
    let status = tokio::process::Command::new(env!("CARGO_BIN_EXE_herdr-white-noise-bridge"))
        .args(["--config", path.to_str().unwrap(), "clear-prompt", &key])
        .status()
        .await
        .unwrap();
    assert!(status.success());
    let mut restarted = Bridge::open(config).unwrap();
    restarted.reconcile(vec![agent()]).await.unwrap();
    assert!(!restarted.state.mappings[&key].pending_prompt);
    assert!(!restarted.state.mappings[&key].completion_pending);
    assert!(restarted.state.mappings[&key].response_id.is_none());
    assert!(
        restarted
            .state
            .outbox
            .iter()
            .any(|item| item.text.contains("cancelled"))
    );
}

#[tokio::test]
async fn a_failed_progress_edit_does_not_block_the_standalone_final() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    export_script(&config.opencode_bin, "Final reply", "final");
    let calls = Arc::new(Mutex::new(vec![]));
    let captured = calls.clone();
    let listener = UnixListener::bind(&config.wn_agent_socket).unwrap();
    let server = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(stream);
            let request = read_line(&mut reader).await.unwrap();
            captured.lock().unwrap().push(request.clone());
            let mut response = if request["type"] == "edit_message" {
                json!({"type":"error","code":"app_error"})
            } else {
                json!({"type":"final_sent","message_ids_hex":["ee".repeat(32)]})
            };
            response["id"] = request["id"].clone();
            response["marmot_agent_control"] = json!("marmot.agent-control.v2");
            reader
                .get_mut()
                .write_all((response.to_string() + "\n").as_bytes())
                .await
                .unwrap();
        }
    });
    let mut bridge = Bridge::open(config).unwrap();
    let key = agent().key();
    let group = "bb".repeat(16);
    bridge.state.mappings.insert(
        key.clone(),
        Mapping {
            group_id_hex: Some(group.clone()),
            agent_record: Some(agent()),
            active: true,
            ..Mapping::default()
        },
    );
    bridge.state.begin_response(&key, &group);
    let progress = bridge.state.outbox.last_mut().unwrap();
    progress.response.as_mut().unwrap().target = Some("cc".repeat(32));
    progress.response.as_mut().unwrap().sent_text = Some("State: working".into());
    bridge.reconcile(vec![agent()]).await.unwrap();
    bridge.flush().await.unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["type"], "send_final");
    assert_eq!(calls[0]["text"], "Final reply");
    assert_eq!(calls[1]["type"], "edit_message");
    assert_eq!(calls[1]["text"], "> done");
    assert!(bridge.state.outbox.is_empty());
    server.abort();
}

#[tokio::test]
async fn a_standalone_final_retries_with_its_own_stable_id_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    export_script(&config.opencode_bin, "Final reply", "final");
    let mut bridge = Bridge::open(config.clone()).unwrap();
    let key = agent().key();
    bridge.state.mappings.insert(
        key,
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            agent_record: Some(agent()),
            active: true,
            ..Mapping::default()
        },
    );
    bridge.reconcile(vec![agent()]).await.unwrap();
    let listener = UnixListener::bind(&config.wn_agent_socket).unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        read_line(&mut tokio::io::BufReader::new(stream))
            .await
            .unwrap()
    });
    assert!(bridge.flush().await.is_err());
    let original = server.await.unwrap();
    std::fs::remove_file(&config.wn_agent_socket).unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let server = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let mut restarted = Bridge::open(config).unwrap();
    restarted.reconcile(vec![agent()]).await.unwrap();
    restarted.flush().await.unwrap();
    restarted.flush().await.unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["type"], "send_final");
    assert_eq!(calls[0]["text"], "Final reply");
    assert_eq!(calls[0]["idempotency_key"], original["idempotency_key"]);
    assert!(restarted.state.outbox.is_empty());
    server.abort();
}

#[tokio::test]
async fn detached_cancelled_progress_cannot_post_working_after_a_final() {
    let dir = tempfile::tempdir().unwrap();
    let config = editable_config(dir.path());
    let mut bridge = Bridge::open(config.clone()).unwrap();
    let key = agent().key();
    let group = "bb".repeat(16);
    bridge.state.mappings.insert(
        key.clone(),
        Mapping {
            group_id_hex: Some(group.clone()),
            ..Mapping::default()
        },
    );
    bridge.state.begin_response(&key, &group);
    // Persisted after an initial send whose response was lost.
    bridge
        .state
        .outbox
        .last_mut()
        .unwrap()
        .response
        .as_mut()
        .unwrap()
        .initial_text = Some("State: working".into());
    bridge
        .state
        .update_response(&key, &group, "Response cancelled locally.", true);
    bridge.state.enqueue(&group, "The next final answer");
    bridge.save().unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let mut restarted = Bridge::open(config).unwrap();
    restarted.flush().await.unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "Do not retry detached progress after the final"
    );
    assert_eq!(calls[0]["type"], "send_final");
    assert_eq!(calls[0]["text"], "The next final answer");
    assert!(restarted.state.outbox.is_empty());
    mdk.abort();
}

#[tokio::test]
async fn real_socket_round_trip_routes_one_group_and_only_new_final_answers() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    export_script(&config.opencode_bin, "historical private answer", "old");
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer(&config.herdr_socket, false, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    bridge.flush().await.unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["type"] == "group_create_with_admins")
            .count(),
        1
    );
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["type"] == "send_final")
            .all(|r| !r["text"]
                .as_str()
                .unwrap()
                .contains("historical private answer"))
    );
    bridge
        .inbound(&inbound(&"ff".repeat(32), "evil"))
        .await
        .unwrap();
    bridge.inbound(&inbound(HEX, "m1")).await.unwrap();
    bridge.inbound(&inbound(HEX, "m1")).await.unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["method"] == "agent.prompt")
            .count(),
        1
    );
    export_script(&config.opencode_bin, "new final", "new");
    let mut a = agent();
    a.state_change_seq = 3;
    bridge.reconcile(vec![a.clone()]).await.unwrap();
    bridge.flush().await.unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["text"] == "new final")
            .count(),
        1
    );
    let mut restarted = Bridge::open(config).unwrap();
    restarted.reconcile(vec![a]).await.unwrap();
    restarted.flush().await.unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["text"] == "new final")
            .count(),
        1
    );
    mdk.abort();
    herdr.abort();
}

#[tokio::test]
async fn working_notice_remains_but_done_only_forwards_the_final_reply() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            last_agent_status: "idle".into(),
            last_state_change_seq: 1,
            final_ids: vec!["old".into()],
            pending_prompt: true,
            agent_record: Some(agent()),
            ..Mapping::default()
        },
    );
    let mut working = agent();
    working.agent_status = "working".into();
    working.state_change_seq = 2;
    bridge.reconcile(vec![working.clone()]).await.unwrap();
    bridge.reconcile(vec![working]).await.unwrap();
    export_script(&config.opencode_bin, "Completed answer", "new");
    let mut done = agent();
    done.agent_status = "done".into();
    done.state_change_seq = 3;
    bridge.reconcile(vec![done.clone()]).await.unwrap();
    bridge.reconcile(vec![done]).await.unwrap();
    let texts: Vec<_> = bridge
        .state
        .outbox
        .iter()
        .map(|m| m.text.as_str())
        .collect();
    assert_eq!(texts.iter().filter(|text| **text == "> working").count(), 1);
    assert!(!texts.contains(&"> done"));
    assert_eq!(
        texts
            .iter()
            .filter(|text| **text == "Completed answer")
            .count(),
        1
    );
    assert_eq!(
        bridge.state.mappings[&agent().key()].last_agent_status,
        "done"
    );
    assert!(!bridge.state.mappings[&agent().key()].pending_prompt);
    assert!(!bridge.state.mappings[&agent().key()].completion_pending);
}

#[tokio::test]
async fn codex_session_round_trip_forwards_only_new_native_final_replies() {
    native_round_trip("codex", false).await;
}

#[tokio::test]
async fn claude_session_round_trip_forwards_only_new_native_final_replies() {
    native_round_trip("claude", false).await;
}

#[tokio::test]
async fn existing_native_groups_seed_history_once_without_recreating_the_group() {
    native_round_trip("codex", true).await;
    native_round_trip("claude", true).await;
}

async fn native_round_trip(provider: &str, legacy_mapping: bool) {
    const ID: &str = "11111111-1111-4111-8111-111111111111";
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let mut observed = agent();
    observed.agent = provider.into();
    observed.agent_session = Some(herdr_white_noise_bridge::herdr::Session {
        source: format!("herdr:{provider}"),
        agent: provider.into(),
        kind: "id".into(),
        value: ID.into(),
    });
    let path = if provider == "codex" {
        config
            .codex_home
            .join("sessions/2026/01/01")
            .join(format!("rollout-test-{ID}.jsonl"))
    } else {
        config
            .claude_home
            .join("projects/project")
            .join(format!("{ID}.jsonl"))
    };
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let write = |new_reply: bool| {
        let mut rows = if provider == "codex" {
            vec![
                json!({"type":"session_meta","payload":{"id":ID}}),
                json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"old","last_agent_message":"Native old final"}}),
            ]
        } else {
            vec![
                json!({"type":"user","uuid":"u0","sessionId":ID,"message":{"role":"user","content":"Old prompt"}}),
                json!({"type":"assistant","uuid":"old","parentUuid":"u0","sessionId":ID,"message":{"role":"assistant","id":"api-old","stop_reason":"end_turn","content":[{"type":"text","text":"Native old final"}]}}),
            ]
        };
        if new_reply {
            if provider == "codex" {
                rows.push(json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"new","last_agent_message":"Native new final"}}));
            } else {
                rows.extend([
                json!({"type":"user","uuid":"u1","parentUuid":"old","sessionId":ID,"message":{"role":"user","content":"New prompt"}}),
                json!({"type":"assistant","uuid":"new","parentUuid":"u1","sessionId":ID,"message":{"role":"assistant","id":"api-new","stop_reason":"end_turn","content":[{"type":"text","text":"Native new final"}]}}),
            ]);
            }
        }
        std::fs::write(
            &path,
            rows.into_iter()
                .map(|row| row.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();
    };
    write(false);
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer_for_agent(&config.herdr_socket, false, calls.clone(), observed.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    if legacy_mapping {
        bridge.state.mappings.insert(
            observed.key(),
            Mapping {
                group_id_hex: Some("bb".repeat(16)),
                active: true,
                agent_record: Some(observed.clone()),
                ..Mapping::default()
            },
        );
    }
    bridge.reconcile(vec![observed.clone()]).await.unwrap();
    bridge.flush().await.unwrap();
    bridge.inbound(&inbound(HEX, "new-prompt")).await.unwrap();
    assert!(bridge.state.mappings[&observed.key()].pending_prompt);
    write(true);
    observed.agent_status = "done".into();
    observed.state_change_seq += 1;
    bridge.reconcile(vec![observed.clone()]).await.unwrap();
    bridge.flush().await.unwrap();
    assert!(!bridge.state.mappings[&observed.key()].pending_prompt);
    let mut restarted = Bridge::open(config).unwrap();
    restarted.reconcile(vec![observed]).await.unwrap();
    restarted.flush().await.unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["method"] == "agent.prompt")
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["text"] == "Native new final")
            .count(),
        1
    );
    assert!(
        !calls
            .iter()
            .any(|call| call["text"] == "Native old final" || call["text"] == "> done")
    );
    if legacy_mapping {
        assert!(
            !calls
                .iter()
                .any(|call| call["type"] == "group_create_with_admins")
        );
    }
    mdk.abort();
    herdr.abort();
}

#[tokio::test]
async fn legacy_native_session_that_exited_before_upgrade_does_not_replay_history() {
    const ID: &str = "11111111-1111-4111-8111-111111111111";
    for provider in ["codex", "claude"] {
        let directory = tempfile::tempdir().unwrap();
        let config = config(directory.path());
        let mut observed = agent();
        observed.agent = provider.into();
        observed.agent_session = Some(herdr_white_noise_bridge::herdr::Session {
            source: format!("herdr:{provider}"),
            agent: provider.into(),
            kind: "id".into(),
            value: ID.into(),
        });
        let (path, rows) = if provider == "codex" {
            (
                config
                    .codex_home
                    .join("sessions/2026/01/01")
                    .join(format!("rollout-test-{ID}.jsonl")),
                vec![
                    json!({"type":"session_meta","payload":{"id":ID}}),
                    json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"old","last_agent_message":"Historical answer"}}),
                ],
            )
        } else {
            (
                config
                    .claude_home
                    .join("projects/project")
                    .join(format!("{ID}.jsonl")),
                vec![
                    json!({"type":"assistant","uuid":"old","sessionId":ID,"message":{"id":"api-old","role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"Historical answer"}]}}),
                ],
            )
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            rows.into_iter()
                .map(|row| row.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();
        let key = observed.key();
        let mut bridge = Bridge::open(config).unwrap();
        bridge.state.mappings.insert(
            key.clone(),
            Mapping {
                group_id_hex: Some("bb".repeat(16)),
                active: true,
                agent_record: Some(observed),
                ..Mapping::default()
            },
        );
        bridge.reconcile(vec![]).await.unwrap();
        assert_eq!(bridge.state.outbox.len(), 1);
        assert!(bridge.state.outbox[0].text.starts_with("> ended"));
        assert!(bridge.state.mappings[&key].output_initialised);
        assert!(!bridge.state.mappings[&key].active);
    }
}

#[tokio::test]
async fn desk_selection_during_the_capability_probe_prevents_a_new_invitation() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    export_script(&config.opencode_bin, "Historical answer", "old");
    let listener = UnixListener::bind(&config.wn_agent_socket).unwrap();
    let captured = Arc::new(Mutex::new(vec![]));
    let calls = captured.clone();
    let control = config.clone();
    let server = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(stream);
            let request = read_line(&mut reader).await.unwrap();
            calls.lock().unwrap().push(request.clone());
            let mut response = if request["type"] == "group_create_capabilities" {
                assert!(forwarding_command(&control, "desk").await.status.success());
                json!({"type":"group_create_capabilities","initial_admins":true})
            } else {
                json!({"type":"group_created","group_id_hex":"bb".repeat(16)})
            };
            response["id"] = request["id"].clone();
            response["marmot_agent_control"] = json!("marmot.agent-control.v2");
            reader
                .get_mut()
                .write_all((response.to_string() + "\n").as_bytes())
                .await
                .unwrap();
        }
    });
    let mut bridge = Bridge::open(config).unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    assert!(bridge.state.mappings[&agent().key()].group_id_hex.is_none());
    assert!(!bridge.state.mappings[&agent().key()].creating);
    assert_eq!(captured.lock().unwrap().len(), 1);
    server.abort();
}

#[tokio::test]
async fn an_old_connector_cannot_leave_creation_intent_that_blocks_its_upgrade() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    export_script(&config.opencode_bin, "Historical answer", "old");
    let listener = UnixListener::bind(&config.wn_agent_socket).unwrap();
    let requests = Arc::new(Mutex::new(vec![]));
    let captured = requests.clone();
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(stream);
            let request = read_line(&mut reader).await.unwrap();
            captured.lock().unwrap().push(request.clone());
            let response = json!({"marmot_agent_control":"marmot.agent-control.v2","id":request["id"],"type":"error","code":"control_error","message":"Unsupported request","retryable":false});
            reader
                .get_mut()
                .write_all((response.to_string() + "\n").as_bytes())
                .await
                .unwrap();
        }
    });
    let mut bridge = Bridge::open(config).unwrap();
    for _ in 0..2 {
        bridge.reconcile(vec![agent()]).await.unwrap();
        let mapping = &bridge.state.mappings[&agent().key()];
        assert!(
            !mapping.creating,
            "An unsupported read-only capability probe is not an uncertain create"
        );
        assert!(mapping.group_id_hex.is_none());
    }
    assert_eq!(requests.lock().unwrap().len(), 2);
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request["type"] == "group_create_capabilities")
    );
    server.abort();
}

#[tokio::test]
async fn uncertain_group_creation_is_journalled_and_never_retried_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    export_script(&config.opencode_bin, "old", "old");
    let listener = UnixListener::bind(&config.wn_agent_socket).unwrap();
    let first = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = tokio::io::BufReader::new(stream);
        let capabilities = read_line(&mut reader).await.unwrap();
        assert_eq!(capabilities["type"], "group_create_capabilities");
        let response = json!({"marmot_agent_control":"marmot.agent-control.v2","id":capabilities["id"],"type":"group_create_capabilities","initial_admins":true});
        reader
            .get_mut()
            .write_all((response.to_string() + "\n").as_bytes())
            .await
            .unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let req = read_line(&mut tokio::io::BufReader::new(stream))
            .await
            .unwrap();
        assert_eq!(req["type"], "group_create_with_admins");
    });
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    first.await.unwrap();
    assert!(BridgeState::load(&config.state_path).unwrap().mappings[&agent().key()].creating);
    let mut restarted = Bridge::open(config).unwrap();
    restarted.reconcile(vec![agent()]).await.unwrap();
    assert!(
        restarted.state.mappings[&agent().key()]
            .group_id_hex
            .is_none()
    );
}

#[test]
fn state_is_private_and_utf8_chunks_preserve_all_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut state = BridgeState::default();
    let text = "🙂".repeat(16000);
    state.enqueue("group", &text);
    state.save(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(state.outbox.iter().all(|i| i.text.len() <= 30000));
    assert_eq!(
        state
            .outbox
            .iter()
            .map(|i| i.text.as_str())
            .collect::<String>(),
        text
    );
}

#[tokio::test]
async fn response_id_mismatch_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream
            .write_all(b"{\"id\":\"wrong\",\"result\":{}}\n")
            .await
            .unwrap();
    });
    assert!(Socket { path }.herdr("ping", json!({})).await.is_err());
    task.await.unwrap();
}

#[tokio::test]
async fn ended_groups_do_not_route_to_a_replacement_agent() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let mut bridge = Bridge::open(config).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            ..Mapping::default()
        },
    );
    bridge.reconcile(vec![]).await.unwrap();
    bridge.inbound(&inbound(HEX, "later")).await.unwrap();
    assert!(
        bridge
            .state
            .outbox
            .iter()
            .any(|m| m.text.contains("No prompt was sent"))
    );
}

#[tokio::test]
async fn a_missing_connector_before_submission_retries_when_it_appears() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    export_script(&config.opencode_bin, "old", "old");
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    assert!(!bridge.state.mappings[&agent().key()].creating);
    let calls = Arc::new(Mutex::new(vec![]));
    let server = peer(&config.wn_agent_socket, true, calls.clone()).await;
    bridge.reconcile(vec![agent()]).await.unwrap();
    assert!(bridge.state.mappings[&agent().key()].group_id_hex.is_some());
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["group_create_capabilities", "group_create_with_admins"]
    );
    server.abort();
}

#[tokio::test]
async fn failed_prompt_preflight_does_not_lock_the_group() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let mut bridge = Bridge::open(config).unwrap();
    bridge.herdr_available = true;
    bridge.agents.insert(agent().key(), agent());
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            ..Mapping::default()
        },
    );
    assert!(bridge.inbound(&inbound(HEX, "m1")).await.is_err());
    assert!(!bridge.state.mappings[&agent().key()].pending_prompt);
    assert!(
        bridge.state.mappings[&agent().key()]
            .seen_messages
            .is_empty()
    );
}

#[tokio::test]
async fn an_outage_defers_phone_input_without_consuming_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut bridge = Bridge::open(config(dir.path())).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            ..Mapping::default()
        },
    );
    assert!(bridge.inbound(&inbound(HEX, "offline")).await.is_err());
    assert!(
        bridge.state.mappings[&agent().key()]
            .seen_messages
            .is_empty()
    );
    assert!(bridge.state.outbox.is_empty());
}

#[tokio::test]
async fn truncated_recovered_input_never_reaches_herdr() {
    let dir = tempfile::tempdir().unwrap();
    let mut bridge = Bridge::open(config(dir.path())).unwrap();
    bridge.herdr_available = true;
    bridge.agents.insert(agent().key(), agent());
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            ..Mapping::default()
        },
    );
    let mut event = inbound(HEX, "long");
    event["message"]["text_truncated"] = json!(true);
    bridge.inbound(&event).await.unwrap();
    assert!(!bridge.state.mappings[&agent().key()].pending_prompt);
    assert!(
        bridge
            .state
            .outbox
            .iter()
            .any(|m| m.text.contains("truncated"))
    );
}

#[tokio::test]
async fn final_output_is_collected_even_if_the_agent_exits_between_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    export_script(&config.opencode_bin, "last answer", "last");
    let mut bridge = Bridge::open(config).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            agent_record: Some(agent()),
            ..Mapping::default()
        },
    );
    bridge.reconcile(vec![]).await.unwrap();
    assert_eq!(bridge.state.outbox[0].text, "last answer");
    assert!(bridge.state.outbox[1].text.starts_with("> ended"));
    bridge.reconcile(vec![]).await.unwrap();
    assert_eq!(bridge.state.outbox.len(), 2);
}

#[tokio::test]
async fn delayed_native_identity_does_not_create_a_provisional_opencode_group() {
    let dir = tempfile::tempdir().unwrap();
    let mut bridge = Bridge::open(config(dir.path())).unwrap();
    let mut a = agent();
    a.agent_session = None;
    bridge.reconcile(vec![a]).await.unwrap();
    assert!(bridge.state.mappings.is_empty());
}

#[tokio::test]
async fn consumed_message_ids_are_not_evicted_while_history_can_replay_them() {
    let dir = tempfile::tempdir().unwrap();
    let mut bridge = Bridge::open(config(dir.path())).unwrap();
    bridge.herdr_available = true;
    let mut event = inbound(HEX, "original");
    event["message"]["text"] = json!("/status");
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            seen_messages: (0..4096).map(|i| i.to_string()).collect(),
            ..Mapping::default()
        },
    );
    bridge.inbound(&event).await.unwrap();
    assert_eq!(bridge.state.mappings[&agent().key()].seen_messages[0], "0");
    assert_eq!(
        bridge.state.mappings[&agent().key()].seen_messages.len(),
        4097
    );
}

#[tokio::test]
async fn failed_session_export_does_not_starve_a_healthy_session() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    std::fs::write(
        &config.opencode_bin,
        "#!/bin/sh\ncase \"$3\" in bad) exit 1;; esac\nprintf '%s' '{\"messages\":[]}'\n",
    )
    .unwrap();
    std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let server = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let mut bridge = Bridge::open(config).unwrap();
    let mut bad = agent();
    bad.agent_session.as_mut().unwrap().value = "bad".into();
    bridge.reconcile(vec![bad, agent()]).await.unwrap();
    assert!(bridge.state.mappings[&agent().key()].group_id_hex.is_some());
    server.abort();
}

#[tokio::test]
async fn recovery_uses_real_cursors_pages_all_history_and_recovers_late_deliveries() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let listener = UnixListener::bind(&config.wn_agent_socket).unwrap();
    let rows: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new((0..60).map(|i|json!({"message_id_hex":format!("{i:064x}"),"recorded_at":100+i,"availability":"available","kind":9,"sender":{"account_id_hex":HEX},"text":"/status","attachments":[]})).collect()));
    let data = rows.clone();
    let server = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(stream);
            let req = read_line(&mut reader).await.unwrap();
            assert_eq!(req["type"], "timeline_list");
            assert!(req.get("after").is_none());
            let mut response = if req["group_id_hex"] == "dd".repeat(16) {
                json!({"type":"error","code":"app_error"})
            } else {
                let rows = data.lock().unwrap();
                let end = if let Some(cursor) = req.get("before") {
                    rows.iter()
                        .position(|row| row["message_id_hex"] == cursor["message_id_hex"])
                        .expect("MDK requires an existing cursor row")
                } else {
                    rows.len()
                };
                let start = end.saturating_sub(50);
                json!({"type":"timeline_page","messages":rows[start..end],"has_more_before":start>0,"has_more_after":false})
            };
            response["id"] = req["id"].clone();
            response["marmot_agent_control"] = json!("marmot.agent-control.v2");
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            reader.get_mut().write_all(&bytes).await.unwrap();
        }
    });
    let mut bridge = Bridge::open(config).unwrap();
    bridge.herdr_available = true;
    bridge.state.mappings.insert(
        "healthy".into(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            joined_at: u64::MAX,
            ..Mapping::default()
        },
    );
    bridge.recover_inbound().await.unwrap();
    assert_eq!(bridge.state.mappings["healthy"].seen_messages.len(), 60);
    let mut late = rows.lock().unwrap()[0].clone();
    late["message_id_hex"] = json!("ee".repeat(32));
    late["recorded_at"] = json!(1);
    rows.lock().unwrap().insert(0, late);
    bridge.state.mappings.insert(
        "failing-first".into(),
        Mapping {
            group_id_hex: Some("dd".repeat(16)),
            ..Mapping::default()
        },
    );
    assert!(bridge.recover_inbound().await.is_err());
    assert_eq!(bridge.state.mappings["healthy"].seen_messages.len(), 61);
    server.abort();
}

#[tokio::test]
async fn failed_export_does_not_repeat_a_lifecycle_notice() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    std::fs::write(&config.opencode_bin, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut bridge = Bridge::open(config).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            last_agent_status: "working".into(),
            agent_record: Some(agent()),
            ..Mapping::default()
        },
    );
    bridge.reconcile(vec![agent()]).await.unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    assert_eq!(
        bridge
            .state
            .outbox
            .iter()
            .filter(|m| m.text == "> idle")
            .count(),
        1
    );
}

#[tokio::test]
async fn temporarily_missing_backend_keeps_an_existing_session_present() {
    let dir = tempfile::tempdir().unwrap();
    let mut bridge = Bridge::open(config(dir.path())).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            agent_record: Some(agent()),
            ..Mapping::default()
        },
    );
    let mut pending = agent();
    pending.agent = String::new();
    bridge.reconcile(vec![pending]).await.unwrap();
    assert!(bridge.state.mappings[&agent().key()].active);
    assert!(
        !bridge
            .state
            .outbox
            .iter()
            .any(|m| m.text.starts_with("> ended"))
    );
}

#[tokio::test]
async fn successful_export_retry_releases_a_completed_prompt_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    std::fs::write(&config.opencode_bin, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&config.opencode_bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            pending_prompt: true,
            last_agent_status: "working".into(),
            agent_record: Some(agent()),
            ..Mapping::default()
        },
    );
    bridge.reconcile(vec![agent()]).await.unwrap();
    assert!(bridge.state.mappings[&agent().key()].completion_pending);
    export_script(&config.opencode_bin, "Recovered final", "final");
    let mut restarted = Bridge::open(config).unwrap();
    restarted.reconcile(vec![agent()]).await.unwrap();
    assert!(!restarted.state.mappings[&agent().key()].pending_prompt);
    assert!(!restarted.state.mappings[&agent().key()].completion_pending);
    assert_eq!(
        restarted
            .state
            .outbox
            .iter()
            .filter(|m| m.text == "Recovered final")
            .count(),
        1
    );
}

#[tokio::test]
async fn unknown_native_identity_never_reuses_an_ended_group_or_consumes_input() {
    let dir = tempfile::tempdir().unwrap();
    let mut bridge = Bridge::open(config(dir.path())).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            active: true,
            agent_record: Some(agent()),
            ..Mapping::default()
        },
    );
    let mut uncertain = agent();
    uncertain.agent_session = None;
    uncertain.agent = "claude".into();
    bridge.reconcile(vec![uncertain]).await.unwrap();
    assert!(bridge.state.mappings[&agent().key()].active);
    assert!(bridge.inbound(&inbound(HEX, "uncertain")).await.is_err());
    assert!(
        bridge.state.mappings[&agent().key()]
            .seen_messages
            .is_empty()
    );
    assert_eq!(bridge.state.mappings.len(), 1);
}

#[tokio::test]
async fn renaming_existing_groups_keeps_their_mapping_and_skips_matching_names() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let listener = UnixListener::bind(&config.wn_agent_socket).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let mdk = tokio::spawn(async move {
        let mut subject = "HERDR · OC | Previous chat topic".to_string();
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(stream);
            let req = read_line(&mut reader).await.unwrap();
            captured.lock().unwrap().push(req.clone());
            let mut response = match req["type"].as_str().unwrap() {
                "group_info" => {
                    json!({"type":"group_info","subject":subject})
                }
                "group_profile_update" => {
                    assert_eq!(req["group_id_hex"], "bb".repeat(16));
                    let description = req["description"].as_str().unwrap();
                    assert!(description.contains("Agent: opencode"));
                    assert!(description.contains("Workspace:"));
                    subject = req["name"].as_str().unwrap().to_owned();
                    json!({"type":"group_profile_updated","message_ids_hex":["cc".repeat(32)]})
                }
                _ => panic!("Group rename must not create or replace a group"),
            };
            response["id"] = req["id"].clone();
            response["marmot_agent_control"] = json!("marmot.agent-control.v2");
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            reader.get_mut().write_all(&bytes).await.unwrap();
        }
    });
    let herdr = peer(&config.herdr_socket, false, Arc::new(Mutex::new(vec![]))).await;
    let mut bridge = Bridge::open(config).unwrap();
    bridge.state.mappings.insert(
        agent().key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            agent_record: Some(agent()),
            ..Mapping::default()
        },
    );
    let original = bridge.state.clone();
    bridge.rename_groups().await.unwrap();
    bridge.rename_groups().await.unwrap();
    assert!(
        bridge.state.mappings[&agent().key()]
            .group_description
            .is_some()
    );
    let mut after = bridge.state.clone();
    after
        .mappings
        .get_mut(&agent().key())
        .unwrap()
        .group_description = None;
    after.mappings.get_mut(&agent().key()).unwrap().group_name = None;
    assert_eq!(after, original);
    let requests = requests.lock().unwrap();
    let updates: Vec<_> = requests
        .iter()
        .filter(|r| r["type"] == "group_profile_update")
        .collect();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["name"], "[cd] Review");
    mdk.abort();
    herdr.abort();
}

#[tokio::test]
async fn renaming_uses_the_existing_title_when_an_ended_group_has_no_stored_tab_label() {
    let directory = tempfile::tempdir().unwrap();
    let config = config(directory.path());
    let calls = Arc::new(Mutex::new(vec![]));
    let herdr = peer(&config.herdr_socket, false, calls.clone()).await;
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let mut old = agent();
    old.tab_id.clear();
    old.tab_label = None;
    old.agent_session.as_mut().unwrap().value = "ses_ended".into();
    let mut bridge = Bridge::open(config).unwrap();
    bridge.state.mappings.insert(
        old.key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            agent_record: Some(old),
            ..Mapping::default()
        },
    );
    bridge.rename_groups().await.unwrap();
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|request| request["type"] == "group_profile_update"
                && request["name"] == "[cd] historical")
    );
    bridge.rename_groups().await.unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request["type"] == "group_profile_update")
            .count(),
        1,
        "Repeating migration must not add another [cd] prefix"
    );
    herdr.abort();
    mdk.abort();
}

#[tokio::test]
async fn deferred_prepared_media_does_not_block_another_session() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    export_script(&config.opencode_bin, "Historical answer", "old");
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer(&config.herdr_socket, false, calls.clone()).await;
    let mut bridge = Bridge::open(config).unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    let mut uncertain = bridge.state.mappings[&agent().key()].clone();
    uncertain.group_id_hex = Some("dd".repeat(16));
    bridge
        .state
        .mappings
        .insert("uncertain-session".into(), uncertain);
    let mut deferred = inbound(HEX, "deferred-media");
    deferred["group_id_hex"] = json!("dd".repeat(16));
    deferred["_codr_prepared_text"] = json!("Deferred prompt");
    let mut ready = inbound(HEX, "ready-media");
    let image = root.path().join("layout.png");
    std::fs::write(&image, b"\x89PNG\r\n\x1a\nexample").unwrap();
    ready["message"]["text"] = json!("Ready prompt");
    ready["message"]["media"] = json!([{"media_type":"image/png","test_path":image,"file_name":"layout.png","source_epoch":1,
        "ciphertext_sha256":"aa".repeat(32),"plaintext_sha256":"bb".repeat(32),"nonce_hex":"00".repeat(12),
        "version":"encrypted-media-v1","source_epoch":1,"locators":[{"kind":"blossom-v1","value":"https://example.com/fixture"}]}]);
    bridge.state.media_pending = vec![deferred, ready];
    assert!(bridge.process_media().await.is_err());
    for _ in 0..100 {
        // Each worker tick can arrive after the deferred session's retry deadline.
        bridge.state.media_pending[0]["_codr_retry_at"] = json!(0);
        let _ = bridge.process_media().await;
        if bridge.state.media_pending.len() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(bridge.state.media_pending.len(), 1);
    assert_eq!(
        bridge.state.media_pending[0]["message"]["message_id_hex"],
        "deferred-media"
    );
    let requests = calls.lock().unwrap();
    let prompts: Vec<_> = requests
        .iter()
        .filter(|call| call["method"] == "agent.prompt")
        .collect();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0]["params"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Ready prompt\n\nAttached image: ")
    );
    mdk.abort();
    herdr.abort();
}

#[tokio::test]
async fn prepared_audio_survives_a_herdr_outage_and_restart() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    export_script(&config.opencode_bin, "Historical answer", "old");
    let command = root.path().join("transcribe");
    std::fs::write(
        &command,
        "#!/bin/sh\nprintf 'Inspect the blue button.\\n'\n",
    )
    .unwrap();
    std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
    config.transcribe_command = vec![command.to_string_lossy().into_owned()];
    let recording = root.path().join("voice.ogg");
    std::fs::write(&recording, "audio fixture").unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer(&config.herdr_socket, false, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    let mut event = inbound(HEX, "audio-outage");
    event["message"]["text"] = json!("");
    event["message"]["media"] = json!([{"media_type":"audio/ogg","test_path":recording,"file_name":"voice.ogg",
        "ciphertext_sha256":"aa".repeat(32),"plaintext_sha256":"bb".repeat(32),"nonce_hex":"00".repeat(12),
        "version":"encrypted-media-v1","source_epoch":1,"locators":[{"kind":"blossom-v1","value":"https://example.com/fixture"}]}]);
    bridge.inbound(&event).await.unwrap();
    bridge.process_media().await.unwrap();
    bridge.herdr_available = false;
    for _ in 0..100 {
        if bridge.process_media().await.is_err() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        BridgeState::load(&config.state_path)
            .unwrap()
            .media_pending
            .len(),
        1
    );
    assert!(
        !bridge.state.mappings[&agent().key()]
            .seen_messages
            .contains(&"audio-outage".into())
    );
    drop(bridge);
    let mut bridge = Bridge::open(config).unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    for _ in 0..100 {
        bridge.process_media().await.unwrap();
        if bridge.state.media_pending.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(bridge.state.media_pending.is_empty());
    bridge.inbound(&event).await.unwrap();
    let requests = calls.lock().unwrap();
    let prompts: Vec<_> = requests
        .iter()
        .filter(|call| call["method"] == "agent.prompt")
        .collect();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0]["params"]["text"], "Inspect the blue button.");
    assert_eq!(
        requests
            .iter()
            .filter(|call| call["type"] == "download_media")
            .count(),
        1
    );
    mdk.abort();
    herdr.abort();
}

#[tokio::test]
async fn a_transcript_notice_survives_lost_send_and_edit_responses_across_restart() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let listener = UnixListener::bind(&config.wn_agent_socket).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let captured = calls.clone();
    let server = tokio::spawn(async move {
        for attempt in 0..4 {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(stream);
            let request = read_line(&mut reader).await.unwrap();
            captured.lock().unwrap().push(request.clone());
            if attempt == 0 || attempt == 2 {
                continue; // The send/edit may have succeeded, but its reply was lost.
            }
            let response = json!({"marmot_agent_control":"marmot.agent-control.v2","id":request["id"],"type":"final_sent","message_ids_hex":["cc".repeat(32)]});
            reader
                .get_mut()
                .write_all((response.to_string() + "\n").as_bytes())
                .await
                .unwrap();
        }
    });
    let mut bridge = Bridge::open(config.clone()).unwrap();
    let id = bridge.state.begin_transcription("bb").unwrap();
    assert!(bridge.flush().await.is_err());
    let item = bridge
        .state
        .outbox
        .iter_mut()
        .find(|item| item.id == id)
        .unwrap();
    item.text = "> transcribed\n\n> A spoken request.".into();
    item.response.as_mut().unwrap().finished = true;
    item.response.as_mut().unwrap().last_attempt_at = 0;
    bridge.save().unwrap();
    drop(bridge);
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.flush().await.unwrap();
    let delivery = bridge.state.outbox[0].response.as_mut().unwrap();
    assert_eq!(delivery.target.as_deref(), Some("cc".repeat(32).as_str()));
    delivery.last_attempt_at = 0;
    assert!(bridge.flush().await.is_err());
    assert_eq!(bridge.state.outbox.len(), 1);
    bridge.state.outbox[0]
        .response
        .as_mut()
        .unwrap()
        .last_attempt_at = 0;
    bridge.save().unwrap();
    drop(bridge);
    let mut bridge = Bridge::open(config).unwrap();
    bridge.flush().await.unwrap();
    assert!(bridge.state.outbox.is_empty());
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[0]["type"], "send_final");
    assert_eq!(calls[1]["type"], "send_final");
    assert_eq!(calls[0]["idempotency_key"], calls[1]["idempotency_key"]);
    assert_eq!(calls[0]["text"], "> transcribing");
    assert_eq!(calls[1]["text"], "> transcribing");
    for call in &calls[2..] {
        assert_eq!(call["type"], "edit_message");
        assert_eq!(call["target_message_id_hex"], "cc".repeat(32));
        assert_eq!(call["text"], "> transcribed\n\n> A spoken request.");
    }
    server.abort();
}

#[tokio::test]
async fn a_transcript_ready_before_its_first_send_uses_one_completed_bubble() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let calls = Arc::new(Mutex::new(vec![]));
    let server = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let mut bridge = Bridge::open(config).unwrap();
    let id = bridge.state.begin_transcription("bb").unwrap();
    let item = bridge
        .state
        .outbox
        .iter_mut()
        .find(|item| item.id == id)
        .unwrap();
    item.text = "> transcribed\n\n> Already prepared.".into();
    item.response.as_mut().unwrap().finished = true;
    bridge.flush().await.unwrap();
    assert!(bridge.state.outbox.is_empty());
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["type"], "send_final");
    assert_eq!(calls[0]["text"], "> transcribed\n\n> Already prepared.");
    server.abort();
}

#[tokio::test]
async fn audio_queue_survives_restart_and_submits_the_transcript_once_without_confirmation() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.edit_responses = true;
    export_script(&config.opencode_bin, "Historical answer", "old");
    let command = root.path().join("transcribe");
    std::fs::write(
        &command,
        "#!/bin/sh\nprintf 'Inspect the blue button.\\n'\n",
    )
    .unwrap();
    std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
    config.transcribe_command = vec![command.to_string_lossy().into_owned()];
    let recording = root.path().join("voice.ogg");
    std::fs::write(&recording, "audio fixture").unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer(&config.herdr_socket, false, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    let mut event = inbound(HEX, "audio-one");
    event["message"]["text"] = json!("");
    event["message"]["media"] = json!([{"media_type":"audio/ogg","test_path":recording,"file_name":"voice.ogg",
        "ciphertext_sha256":"aa".repeat(32),"plaintext_sha256":"bb".repeat(32),"nonce_hex":"00".repeat(12),
        "version":"encrypted-media-v1","source_epoch":1,"locators":[{"kind":"blossom-v1","value":"https://example.com/fixture"}]}]);
    let mut foreign = event.clone();
    foreign["message"]["sender"]["account_id_hex"] = json!("ff".repeat(32));
    bridge.inbound(&foreign).await.unwrap();
    assert!(bridge.state.media_pending.is_empty());
    let mut summary = event.clone();
    summary["message"]["media"] = json!([{"media_type":"audio/ogg","file_name":"voice.ogg"}]);
    bridge.inbound(&summary).await.unwrap();
    bridge.inbound(&summary).await.unwrap();
    assert_eq!(bridge.state.media_pending.len(), 1);
    assert!(
        !bridge.state.mappings[&agent().key()]
            .seen_messages
            .contains(&"audio-one".into())
    );
    bridge.flush().await.unwrap();
    drop(bridge);
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    for _ in 0..2 {
        for _ in 0..100 {
            bridge.process_media().await.unwrap();
            if bridge.state.media_pending[0]["_codr_retry_at"]
                .as_u64()
                .unwrap_or(0)
                > now()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            bridge.state.media_pending[0]["_codr_retry_at"]
                .as_u64()
                .unwrap_or(0)
                > now()
        );
        assert!(
            !bridge.state.mappings[&agent().key()]
                .seen_messages
                .contains(&"audio-one".into())
        );
        for _ in 0..3 {
            bridge.inbound(&summary).await.unwrap();
            bridge.process_media().await.unwrap();
        }
        assert_eq!(
            bridge
                .state
                .outbox
                .iter()
                .filter(|m| m.text == "> transcribing")
                .count(),
            1,
            "Unavailable history references must not repeat the preparation notice"
        );
        bridge.state.media_pending[0]["_codr_retry_at"] = json!(0);
    }
    bridge.process_media().await.unwrap();
    // An old MDK returns summaries during recovery. A complete live event must
    // hydrate that queued row, not be discarded as a duplicate.
    bridge.inbound(&event).await.unwrap();
    for _ in 0..100 {
        bridge.process_media().await.unwrap();
        if bridge.state.media_pending.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(bridge.state.media_pending.is_empty());
    assert!(
        bridge.state.mappings[&agent().key()]
            .seen_messages
            .contains(&"audio-one".into())
    );
    bridge.inbound(&event).await.unwrap();
    // The original bubble target and completed transcript edit survive restart.
    for item in &mut bridge.state.outbox {
        if let Some(delivery) = &mut item.response {
            delivery.last_attempt_at = 0;
        }
    }
    bridge.save().unwrap();
    drop(bridge);
    let mut bridge = Bridge::open(config).unwrap();
    bridge.flush().await.unwrap();
    let requests = calls.lock().unwrap();
    let prompts: Vec<_> = requests
        .iter()
        .filter(|call| call["method"] == "agent.prompt")
        .collect();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0]["params"]["text"], "Inspect the blue button.");
    assert_eq!(
        requests
            .iter()
            .filter(|call| call["type"] == "send_final" && call["text"] == "> transcribing")
            .count(),
        1
    );
    let edits: Vec<_> = requests
        .iter()
        .filter(|call| {
            call["type"] == "edit_message"
                && call["text"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("> transcribed"))
        })
        .collect();
    assert_eq!(edits.len(), 1);
    assert_eq!(
        edits[0]["text"],
        "> transcribed\n\n> Inspect the blue button."
    );
    assert_eq!(edits[0]["target_message_id_hex"], "cc".repeat(32));
    assert_eq!(
        requests
            .iter()
            .filter(|call| call["type"] == "download_media")
            .count(),
        1
    );
    mdk.abort();
    herdr.abort();
}

struct GroupProfile {
    subject: String,
    description: String,
    reject_updates: bool,
}

async fn group_profile_peer(
    path: &std::path::Path,
    profile: Arc<Mutex<GroupProfile>>,
    calls: Arc<Mutex<Vec<Value>>>,
    state_path: std::path::PathBuf,
) -> tokio::task::JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(stream);
            let req = read_line(&mut reader).await.unwrap();
            calls.lock().unwrap().push(req.clone());
            let mut response = match req["type"].as_str().unwrap() {
                "group_info" => {
                    let profile = profile.lock().unwrap();
                    json!({"type":"group_info","subject":profile.subject,"description":profile.description})
                }
                "group_profile_update" => {
                    let saved: BridgeState =
                        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
                    assert!(
                        saved
                            .mappings
                            .values()
                            .any(|mapping| mapping.rename_uncertain)
                    );
                    let mut profile = profile.lock().unwrap();
                    if profile.reject_updates {
                        json!({"type":"error","error":"uncertain"})
                    } else {
                        profile.subject = req["name"].as_str().unwrap().to_owned();
                        profile.description = req["description"].as_str().unwrap().to_owned();
                        json!({"type":"group_profile_updated","message_ids_hex":["ee".repeat(32)]})
                    }
                }
                other => panic!("unexpected group profile request: {other}"),
            };
            response["id"] = req["id"].clone();
            response["marmot_agent_control"] = json!("marmot.agent-control.v2");
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            reader.get_mut().write_all(&bytes).await.unwrap();
        }
    })
}

fn mapped_bridge(config: BridgeConfig, observed: &Agent) -> Bridge {
    let mut bridge = Bridge::open(config).unwrap();
    bridge.state.mappings.insert(
        observed.key(),
        Mapping {
            group_id_hex: Some("bb".repeat(16)),
            group_name: Some(observed.group_name()),
            group_description: Some(observed.group_description()),
            output_initialised: true,
            last_agent_status: "idle".into(),
            last_state_change_seq: observed.state_change_seq,
            last_export_at: now(),
            active: true,
            agent_record: Some(observed.clone()),
            ..Mapping::default()
        },
    );
    bridge
}

#[tokio::test]
async fn a_tab_rename_updates_the_group_once() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let mut observed = agent();
    observed.tab_label = Some("Review".into());
    let profile = Arc::new(Mutex::new(GroupProfile {
        subject: observed.group_name(),
        description: observed.group_description(),
        reject_updates: false,
    }));
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = group_profile_peer(
        &config.wn_agent_socket,
        profile,
        calls.clone(),
        config.state_path.clone(),
    )
    .await;
    let mut bridge = mapped_bridge(config, &observed);
    let mut renamed = observed.clone();
    renamed.tab_label = Some("Renamed".into());
    bridge.reconcile(vec![renamed.clone()]).await.unwrap();
    bridge.reconcile(vec![renamed]).await.unwrap();
    let updates: Vec<_> = calls
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request["type"] == "group_profile_update")
        .cloned()
        .collect();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["name"], "[cd] Renamed");
    assert_eq!(
        bridge.state.mappings[&observed.key()].group_name.as_deref(),
        Some("[cd] Renamed")
    );
    mdk.abort();
}

#[tokio::test]
async fn a_group_renamed_by_the_user_is_not_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let mut observed = agent();
    observed.tab_label = Some("Review".into());
    let profile = Arc::new(Mutex::new(GroupProfile {
        subject: "My project".into(),
        description: observed.group_description(),
        reject_updates: false,
    }));
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = group_profile_peer(
        &config.wn_agent_socket,
        profile,
        calls.clone(),
        config.state_path.clone(),
    )
    .await;
    let mut bridge = mapped_bridge(config, &observed);
    let mut renamed = observed.clone();
    renamed.tab_label = Some("Renamed".into());
    bridge.reconcile(vec![renamed]).await.unwrap();
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .all(|request| request["type"] != "group_profile_update")
    );
    assert!(bridge.state.mappings[&observed.key()].group_name_locked);
    mdk.abort();
}

#[tokio::test]
async fn an_unconfirmed_profile_update_is_not_retried() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let mut observed = agent();
    observed.tab_label = Some("Review".into());
    let profile = Arc::new(Mutex::new(GroupProfile {
        subject: observed.group_name(),
        description: observed.group_description(),
        reject_updates: true,
    }));
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = group_profile_peer(
        &config.wn_agent_socket,
        profile,
        calls.clone(),
        config.state_path.clone(),
    )
    .await;
    let mut bridge = mapped_bridge(config.clone(), &observed);
    let mut renamed = observed.clone();
    renamed.tab_label = Some("Renamed".into());
    bridge.reconcile(vec![renamed.clone()]).await.unwrap();
    drop(bridge);
    let mut bridge = Bridge::open(config).unwrap();
    bridge.reconcile(vec![renamed]).await.unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request["type"] == "group_profile_update")
            .count(),
        1
    );
    assert!(bridge.state.mappings[&observed.key()].rename_uncertain);
    mdk.abort();
}

fn claude_agent(status: &str) -> Agent {
    let mut observed = agent();
    observed.agent = "claude".into();
    observed.agent_status = status.into();
    observed.agent_session = Some(herdr_white_noise_bridge::herdr::Session {
        source: "herdr:claude".into(),
        agent: "claude".into(),
        kind: "id".into(),
        value: "11111111-1111-4111-8111-111111111111".into(),
    });
    observed
}

/// Rows are (uuid, kind, text). Kind is "user", "queued" or "final".
fn claude_transcript(config: &BridgeConfig, rows: &[(&str, &str, &str)]) {
    const ID: &str = "11111111-1111-4111-8111-111111111111";
    let path = config
        .claude_home
        .join("projects/project")
        .join(format!("{ID}.jsonl"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut parent = Value::Null;
    let mut text = String::new();
    for (uuid, kind, body) in rows {
        let row = match *kind {
            "user" => {
                json!({"type":"user","uuid":uuid,"parentUuid":parent,"sessionId":ID,"message":{"role":"user","content":body}})
            }
            "queued" => {
                json!({"type":"attachment","uuid":uuid,"parentUuid":parent,"sessionId":ID,"attachment":{"type":"queued_command","prompt":body,"commandMode":"prompt","origin":{"kind":"human"}}})
            }
            _ => {
                json!({"type":"assistant","uuid":uuid,"parentUuid":parent,"sessionId":ID,"message":{"role":"assistant","id":format!("api-{uuid}"),"stop_reason":"end_turn","content":[{"type":"text","text":body}]}})
            }
        };
        text.push_str(&(row.to_string() + "\n"));
        parent = json!(uuid);
    }
    std::fs::write(path, text).unwrap();
}

fn sent(calls: &Arc<Mutex<Vec<Value>>>, text: &str) -> usize {
    calls
        .lock()
        .unwrap()
        .iter()
        .filter(|call| call["type"] == "send_final" && call["text"] == text)
        .count()
}

async fn phone_only_bridge(
    status: &str,
) -> (
    tempfile::TempDir,
    BridgeConfig,
    Agent,
    Arc<Mutex<Vec<Value>>>,
    Bridge,
    [tokio::task::JoinHandle<()>; 2],
) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(dir.path());
    config.phone_replies_only = true;
    let observed = claude_agent(status);
    claude_transcript(
        &config,
        &[("u0", "user", "Old prompt"), ("a0", "final", "Old final")],
    );
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer_for_agent(&config.herdr_socket, false, calls.clone(), observed.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.reconcile(vec![observed.clone()]).await.unwrap();
    bridge.flush().await.unwrap();
    (dir, config, observed, calls, bridge, [mdk, herdr])
}

#[tokio::test]
async fn phone_replies_only_sends_the_reply_to_a_phone_prompt_and_drops_peer_replies() {
    let (_dir, config, mut observed, calls, mut bridge, peers) = phone_only_bridge("idle").await;
    claude_transcript(
        &config,
        &[
            ("u0", "user", "Old prompt"),
            ("a0", "final", "Old final"),
            ("u1", "user", "From: a peer agent"),
            ("a1", "final", "Reply to the peer"),
        ],
    );
    observed.state_change_seq += 1;
    bridge.reconcile(vec![observed.clone()]).await.unwrap();
    bridge.flush().await.unwrap();
    bridge.inbound(&inbound(HEX, "phone-1")).await.unwrap();
    claude_transcript(
        &config,
        &[
            ("u0", "user", "Old prompt"),
            ("a0", "final", "Old final"),
            ("u1", "user", "From: a peer agent"),
            ("a1", "final", "Reply to the peer"),
            ("u2", "user", "test prompt"),
            ("a2", "final", "Reply to the phone"),
        ],
    );
    observed.state_change_seq += 1;
    bridge.reconcile(vec![observed.clone()]).await.unwrap();
    bridge.flush().await.unwrap();
    drop(bridge);
    let mut restarted = Bridge::open(config).unwrap();
    restarted.reconcile(vec![observed]).await.unwrap();
    restarted.flush().await.unwrap();
    assert_eq!(sent(&calls, "Reply to the phone"), 1);
    assert_eq!(sent(&calls, "Reply to the peer"), 0);
    assert_eq!(sent(&calls, "Old final"), 0);
    for handle in peers {
        handle.abort();
    }
}

#[tokio::test]
async fn phone_replies_only_answers_a_prompt_typed_while_the_agent_works() {
    // Claude Code either injects the held prompt into the running turn or opens a new
    // turn with it after the running turn ends.
    for injected in [true, false] {
        let (_dir, config, mut observed, calls, mut bridge, peers) =
            phone_only_bridge("working").await;
        claude_transcript(
            &config,
            &[
                ("u0", "user", "Old prompt"),
                ("a0", "final", "Old final"),
                ("u1", "user", "From: a peer agent"),
            ],
        );
        bridge.reconcile(vec![observed.clone()]).await.unwrap();
        bridge.inbound(&inbound(HEX, "phone-busy")).await.unwrap();
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| call["method"] == "agent.prompt")
                .count(),
            1,
            "injected={injected}"
        );
        let mut rows = vec![
            ("u0", "user", "Old prompt"),
            ("a0", "final", "Old final"),
            ("u1", "user", "From: a peer agent"),
        ];
        if injected {
            rows.extend([
                ("q1", "queued", "test prompt"),
                ("a1", "final", "Reply to the peer and the phone"),
            ]);
        } else {
            rows.extend([
                ("a1", "final", "Reply to the peer"),
                ("u2", "user", "test prompt"),
                ("a2", "final", "Reply to the phone"),
            ]);
        }
        claude_transcript(&config, &rows);
        observed.agent_status = "idle".into();
        observed.state_change_seq += 1;
        bridge.reconcile(vec![observed.clone()]).await.unwrap();
        bridge.flush().await.unwrap();
        if injected {
            assert_eq!(sent(&calls, "Reply to the peer and the phone"), 1);
        } else {
            assert_eq!(sent(&calls, "Reply to the phone"), 1);
            assert_eq!(sent(&calls, "Reply to the peer"), 0);
        }
        for handle in peers {
            handle.abort();
        }
    }
}

#[tokio::test]
async fn without_phone_replies_only_a_peer_reply_still_reaches_the_phone() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let mut observed = claude_agent("idle");
    claude_transcript(
        &config,
        &[("u0", "user", "Old prompt"), ("a0", "final", "Old final")],
    );
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer_for_agent(&config.herdr_socket, false, calls.clone(), observed.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    bridge.reconcile(vec![observed.clone()]).await.unwrap();
    claude_transcript(
        &config,
        &[
            ("u0", "user", "Old prompt"),
            ("a0", "final", "Old final"),
            ("u1", "user", "From: a peer agent"),
            ("q1", "queued", "Another peer message"),
            ("a1", "final", "Reply to the peer"),
        ],
    );
    observed.state_change_seq += 1;
    bridge.reconcile(vec![observed]).await.unwrap();
    bridge.flush().await.unwrap();
    assert_eq!(sent(&calls, "Reply to the peer"), 1);
    mdk.abort();
    herdr.abort();
}

async fn step(bridge: &mut Bridge, observed: &mut Agent, status: &str) {
    observed.agent_status = status.into();
    observed.state_change_seq += 1;
    bridge.reconcile(vec![observed.clone()]).await.unwrap();
    bridge.flush().await.unwrap();
}

#[tokio::test]
async fn phone_replies_only_sends_status_notices_only_for_phone_prompts() {
    let (_dir, config, mut observed, calls, mut bridge, peers) = phone_only_bridge("idle").await;
    let notices = |calls: &Arc<Mutex<Vec<Value>>>| -> Vec<String> {
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call["type"] == "send_final")
            .filter_map(|call| call["text"].as_str())
            .filter(|text| text.starts_with("> "))
            .map(str::to_owned)
            .collect()
    };
    let peer_turn = [
        ("u0", "user", "Old prompt"),
        ("a0", "final", "Old final"),
        ("u1", "user", "From: a peer agent"),
    ];
    claude_transcript(&config, &peer_turn);
    step(&mut bridge, &mut observed, "working").await;
    step(&mut bridge, &mut observed, "blocked").await;
    step(&mut bridge, &mut observed, "working").await;
    let mut answered = peer_turn.to_vec();
    answered.push(("a1", "final", "Reply to the peer"));
    claude_transcript(&config, &answered);
    step(&mut bridge, &mut observed, "idle").await;
    let peer_notices = notices(&calls);
    assert!(
        peer_notices
            .iter()
            .all(|text| text.starts_with("> blocked")),
        "{peer_notices:?}"
    );
    assert!(!peer_notices.is_empty());
    bridge.inbound(&inbound(HEX, "phone-status")).await.unwrap();
    step(&mut bridge, &mut observed, "working").await;
    answered.extend([
        ("u2", "user", "test prompt"),
        ("a2", "final", "Reply to the phone"),
    ]);
    claude_transcript(&config, &answered);
    step(&mut bridge, &mut observed, "idle").await;
    let phone_notices: Vec<_> = notices(&calls).split_off(peer_notices.len());
    assert_eq!(phone_notices, ["> working", "> idle"]);
    assert_eq!(sent(&calls, "Reply to the phone"), 1);
    assert_eq!(sent(&calls, "Reply to the peer"), 0);
    let mut next = answered.clone();
    next.push(("u3", "user", "From: another peer"));
    claude_transcript(&config, &next);
    step(&mut bridge, &mut observed, "working").await;
    assert_eq!(notices(&calls).len(), peer_notices.len() + 2);
    for handle in peers {
        handle.abort();
    }
}

#[tokio::test]
async fn a_phone_prompt_reaches_a_working_agent_and_a_blocked_agent_refuses_it() {
    for status in ["working", "blocked"] {
        let root = tempfile::tempdir().unwrap();
        let config = config(root.path());
        export_script(&config.opencode_bin, "Historical answer", "old");
        let calls = Arc::new(Mutex::new(vec![]));
        let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
        let herdr = peer(&config.herdr_socket, false, calls.clone()).await;
        let mut bridge = Bridge::open(config).unwrap();
        let mut observed = agent();
        observed.agent_status = status.into();
        bridge.reconcile(vec![observed]).await.unwrap();
        bridge.state.outbox.clear();
        bridge.inbound(&inbound(HEX, "prompt")).await.unwrap();
        let notices: Vec<_> = bridge
            .state
            .outbox
            .iter()
            .map(|item| item.text.as_str())
            .collect();
        let prompts = calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call["method"] == "agent.prompt")
            .count();
        if status == "blocked" {
            assert_eq!(prompts, 0);
            assert_eq!(
                notices,
                ["The agent is waiting on a terminal prompt. Answer it at the desk, then resend."]
            );
        } else {
            assert_eq!(prompts, 1);
            assert!(notices.is_empty(), "{notices:?}");
        }
        mdk.abort();
        herdr.abort();
    }
}

#[tokio::test]
async fn a_phone_prompt_behind_media_from_the_same_group_gets_the_media_notice() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    export_script(&config.opencode_bin, "Historical answer", "old");
    let calls = Arc::new(Mutex::new(vec![]));
    let mdk = peer(&config.wn_agent_socket, true, calls.clone()).await;
    let herdr = peer(&config.herdr_socket, false, calls.clone()).await;
    let mut bridge = Bridge::open(config).unwrap();
    bridge.reconcile(vec![agent()]).await.unwrap();
    bridge.state.outbox.clear();
    bridge.state.media_pending = vec![inbound(HEX, "media")];
    bridge.inbound(&inbound(HEX, "prompt")).await.unwrap();
    let notices: Vec<_> = bridge
        .state
        .outbox
        .iter()
        .map(|item| item.text.as_str())
        .collect();
    assert_eq!(
        notices,
        ["Media from this group is still being prepared. Wait for it, then resend."]
    );
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call["method"] == "agent.prompt")
    );
    mdk.abort();
    herdr.abort();
}
