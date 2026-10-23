use bech32::ToBase32;
use herdr_white_noise_bridge::{
    BridgeConfig,
    bridge::Bridge,
    config::Backend,
    openrig::{self, Action, Event},
    wire::read_line,
};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UnixListener},
    sync::mpsc,
};

// Synthetic test identity. Never use it as an installation owner.
const HEX: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const SEAT: &str = "dev-build@workshop";
const FIRST: &str = "11111111-1111-4111-8111-111111111111";
const SECOND: &str = "22222222-2222-4222-8222-222222222222";
const GROUP: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const BLOCKED: &str =
    "The agent is waiting on a terminal prompt. Answer it at the desk, then resend.";

fn phone_npub() -> String {
    bech32::encode("npub", [0x11u8; 32].to_base32(), bech32::Variant::Bech32).unwrap()
}

/// A fake `rig` that logs each call and prints the canned `ps.json` or `send.json`.
fn config(dir: &Path) -> BridgeConfig {
    let rig = dir.join("rig");
    std::fs::write(
        &rig,
        "#!/bin/sh\n\
         dir=$(dirname \"$0\")\n\
         printf '%s\\n' \"$*\" >> \"$dir/rig.log\"\n\
         case \"$1\" in\n\
         ps) cat \"$dir/ps.json\" ;;\n\
         send) cat \"$dir/send.json\"; exit \"$(cat \"$dir/send.exit\")\" ;;\n\
         *) exit 9 ;;\n\
         esac\n",
    )
    .unwrap();
    std::fs::set_permissions(&rig, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = BridgeConfig::from_toml(&format!(
        "phone_npub = {:?}\nbackend = \"openrig\"\nopenrig_seats = [{SEAT:?}]\nrig_bin = {:?}\n\
         wn_agent_socket = {:?}\naccount_id_hex = {:?}\nstate_path = {:?}\nclaude_home = {:?}\n\
         codex_home = {:?}\n",
        phone_npub(),
        rig,
        dir.join("wn.sock"),
        "aa".repeat(32),
        dir.join("state.json"),
        dir.join("claude"),
        dir.join("codex"),
    ))
    .unwrap();
    send_reply(dir, json!({"ok":true,"sessionName":SEAT,"sent":true}), 0);
    config
}

fn node(seat: &str, token: &str, state: &str, seq: u64) -> Value {
    json!({"canonicalSessionName":seat,"rigName":"workshop","runtime":"claude-code",
        "sessionStatus":"running","resumeType":"claude_id","resumeToken":token,"cwd":"/work/project",
        "agentActivity":{"state":state},
        "activityState":{"display":if state == "running" {"working"} else {"idle"},"needsInput":{"count":0},"seq":seq}})
}

fn seats(dir: &Path, nodes: &[Value]) {
    std::fs::write(dir.join("ps.json"), Value::from(nodes.to_vec()).to_string()).unwrap();
}

fn send_reply(dir: &Path, reply: Value, code: i32) {
    std::fs::write(dir.join("send.json"), reply.to_string()).unwrap();
    std::fs::write(dir.join("send.exit"), code.to_string()).unwrap();
}

fn rig_calls(dir: &Path, command: &str) -> Vec<String> {
    std::fs::read_to_string(dir.join("rig.log"))
        .unwrap_or_default()
        .lines()
        .filter(|line| line.starts_with(command))
        .map(str::to_owned)
        .collect()
}

/// Rows are (uuid, kind, text). Kind is "user" or "final".
fn transcript(config: &BridgeConfig, id: &str, rows: &[(&str, &str, &str)]) {
    let path = config
        .claude_home
        .join("projects/project")
        .join(format!("{id}.jsonl"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut parent = Value::Null;
    let mut text = String::new();
    for (uuid, kind, body) in rows {
        let row = if *kind == "user" {
            json!({"type":"user","uuid":uuid,"parentUuid":parent,"sessionId":id,"message":{"role":"user","content":body}})
        } else {
            json!({"type":"assistant","uuid":uuid,"parentUuid":parent,"sessionId":id,"message":{"role":"assistant","id":format!("api-{uuid}"),"stop_reason":"end_turn","content":[{"type":"text","text":body}]}})
        };
        text.push_str(&(row.to_string() + "\n"));
        parent = json!(uuid);
    }
    std::fs::write(path, text).unwrap();
}

fn inbound(id: &str, text: &str) -> Value {
    json!({"marmot_agent_control":"marmot.agent-control.v2","type":"inbound_message","account_id_hex":"aa".repeat(32),"group_id_hex":GROUP,"message":{"message_id_hex":id,"sender":{"account_id_hex":HEX},"text":text,"recorded_at":u64::MAX}})
}

async fn mdk(path: &Path, calls: Arc<Mutex<Vec<Value>>>) -> tokio::task::JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(stream);
            let req = read_line(&mut reader).await.unwrap();
            calls.lock().unwrap().push(req.clone());
            let mut response = match req["type"].as_str().unwrap() {
                "group_create_capabilities" => {
                    json!({"type":"group_create_capabilities","initial_admins":true})
                }
                "group_create_with_admins" => json!({"type":"group_created","group_id_hex":GROUP}),
                "send_final" => json!({"type":"final_sent","message_ids_hex":["cc".repeat(32)]}),
                _ => panic!("unexpected request"),
            };
            response["id"] = req["id"].clone();
            response["marmot_agent_control"] = json!("marmot.agent-control.v2");
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            reader.get_mut().write_all(&bytes).await.unwrap();
        }
    })
}

fn sent(calls: &Arc<Mutex<Vec<Value>>>, text: &str) -> usize {
    calls
        .lock()
        .unwrap()
        .iter()
        .filter(|call| call["type"] == "send_final" && call["text"] == text)
        .count()
}

async fn tick(bridge: &mut Bridge) {
    let agents = bridge.snapshot().await.unwrap();
    bridge.reconcile(agents).await.unwrap();
    bridge.flush().await.unwrap();
}

async fn connected_seat() -> (
    tempfile::TempDir,
    BridgeConfig,
    Arc<Mutex<Vec<Value>>>,
    Bridge,
    tokio::task::JoinHandle<()>,
) {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    transcript(
        &config,
        FIRST,
        &[("u0", "user", "Old prompt"), ("a0", "final", "Old final")],
    );
    seats(dir.path(), &[node(SEAT, FIRST, "idle", 1)]);
    let calls = Arc::new(Mutex::new(vec![]));
    let peer = mdk(&config.wn_agent_socket, calls.clone()).await;
    let mut bridge = Bridge::open(config.clone()).unwrap();
    tick(&mut bridge).await;
    (dir, config, calls, bridge, peer)
}

#[tokio::test]
async fn discovery_maps_listed_running_seats_to_agents() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let mut stopped = node("dev-qa@workshop", SECOND, "idle", 1);
    stopped["sessionStatus"] = json!("stopped");
    let mut blocked = node(SEAT, FIRST, "needs_input", 7);
    blocked["activityState"]["needsInput"]["count"] = json!(1);
    seats(
        dir.path(),
        &[
            blocked,
            node("orch-lead@workshop", SECOND, "running", 3),
            stopped,
        ],
    );
    let mut config = config;
    config.openrig_seats.push("dev-qa@workshop".into());
    let bridge = Bridge::open(config).unwrap();
    let agents = bridge.snapshot().await.unwrap();
    assert_eq!(agents.len(), 1);
    let agent = &agents[0];
    assert_eq!(agent.key(), json!(["openrig", SEAT]).to_string());
    assert_eq!(agent.agent, "claude");
    assert_eq!(agent.agent_status, "blocked");
    assert_eq!(agent.state_change_seq, 7);
    assert_eq!(agent.cwd, "/work/project");
    assert_eq!(agent.agent_session.as_ref().unwrap().value, FIRST);
    assert_eq!(agent.group_name(), format!("[cd] {SEAT}"));
    assert!(
        agent
            .group_description()
            .ends_with("Managed by codr via OpenRig.")
    );
    assert_eq!(rig_calls(dir.path(), "ps"), ["ps --nodes -A --json --full"]);

    seats(
        dir.path(),
        &[
            node(SEAT, FIRST, "running", 8),
            node("dev-qa@workshop", SECOND, "idle", 2),
        ],
    );
    let agents = bridge.snapshot().await.unwrap();
    let status: Vec<_> = agents
        .iter()
        .map(|agent| agent.agent_status.as_str())
        .collect();
    assert_eq!(status, ["working", "idle"]);
}

#[tokio::test]
async fn a_phone_prompt_goes_through_rig_send_and_a_state_change_forwards_the_final() {
    let (dir, config, calls, mut bridge, peer) = connected_seat().await;
    assert_eq!(
        sent(
            &calls,
            &format!(
                "Connected to OpenRig: {SEAT}. Send a message here to prompt this session. Use /status for its state."
            )
        ),
        1
    );
    bridge
        .inbound(&inbound("phone-1", "-n fix the test"))
        .await
        .unwrap();
    assert_eq!(
        rig_calls(dir.path(), "send"),
        [format!("send {SEAT} --raw --json -- -n fix the test")]
    );
    transcript(
        &config,
        FIRST,
        &[
            ("u0", "user", "Old prompt"),
            ("a0", "final", "Old final"),
            ("u1", "user", "-n fix the test"),
            ("a1", "final", "Fixed"),
        ],
    );
    seats(dir.path(), &[node(SEAT, FIRST, "running", 2)]);
    tick(&mut bridge).await;
    seats(dir.path(), &[node(SEAT, FIRST, "idle", 3)]);
    tick(&mut bridge).await;
    assert_eq!(sent(&calls, "Fixed"), 1);
    assert_eq!(sent(&calls, "Old final"), 0);
    bridge
        .inbound(&inbound("phone-2", "/status"))
        .await
        .unwrap();
    bridge.flush().await.unwrap();
    assert_eq!(sent(&calls, "> idle OpenRig session: connected."), 1);
    peer.abort();
}

#[tokio::test]
async fn a_new_session_on_the_seat_keeps_the_group_and_reads_the_new_transcript() {
    let (dir, config, calls, mut bridge, peer) = connected_seat().await;
    let key = json!(["openrig", SEAT]).to_string();
    // A resumed session carries history that the phone has never seen.
    transcript(
        &config,
        SECOND,
        &[
            ("r0", "user", "Earlier work"),
            ("r1", "final", "Earlier final"),
        ],
    );
    seats(dir.path(), &[node(SEAT, SECOND, "idle", 2)]);
    tick(&mut bridge).await;
    assert_eq!(bridge.state.mappings.len(), 1);
    assert_eq!(
        bridge.state.mappings[&key].group_id_hex.as_deref(),
        Some(GROUP)
    );
    assert_eq!(sent(&calls, "Earlier final"), 0);
    assert!(bridge.state.forwarding_enabled);
    bridge
        .inbound(&inbound("phone-1", "Next step"))
        .await
        .unwrap();
    transcript(
        &config,
        SECOND,
        &[
            ("r0", "user", "Earlier work"),
            ("r1", "final", "Earlier final"),
            ("r2", "user", "Next step"),
            ("r3", "final", "Next final"),
        ],
    );
    seats(dir.path(), &[node(SEAT, SECOND, "running", 3)]);
    tick(&mut bridge).await;
    seats(dir.path(), &[node(SEAT, SECOND, "idle", 4)]);
    tick(&mut bridge).await;
    assert_eq!(sent(&calls, "Next final"), 1);
    assert_eq!(sent(&calls, "Earlier final"), 0);
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call["type"] == "group_create_with_admins")
            .count(),
        1
    );
    peer.abort();
}

#[tokio::test]
async fn a_rig_send_refusal_at_an_interactive_prompt_tells_the_phone_and_is_not_retried() {
    let (dir, _config, calls, mut bridge, peer) = connected_seat().await;
    send_reply(
        dir.path(),
        json!({"ok":false,"sessionName":SEAT,"reason":"target_needs_input",
            "error":"The target is waiting on an interactive prompt. No text was sent."}),
        1,
    );
    bridge.inbound(&inbound("phone-1", "go on")).await.unwrap();
    bridge.flush().await.unwrap();
    assert_eq!(sent(&calls, BLOCKED), 1);
    let key = json!(["openrig", SEAT]).to_string();
    assert!(!bridge.state.mappings[&key].pending_prompt);
    bridge.inbound(&inbound("phone-1", "go on")).await.unwrap();
    assert_eq!(rig_calls(dir.path(), "send").len(), 1);

    send_reply(
        dir.path(),
        json!({"ok":false,"sessionName":SEAT,"sent":false,"reason":"typing_guard_enabled",
            "error":"Typing guard prevents submit-only; no Enter was sent."}),
        1,
    );
    bridge.inbound(&inbound("phone-3", "go on")).await.unwrap();
    bridge.flush().await.unwrap();
    assert_eq!(
        sent(
            &calls,
            "OpenRig did not type the prompt (typing_guard_enabled). Resend it when the seat accepts input."
        ),
        1
    );
    bridge.inbound(&inbound("phone-3", "go on")).await.unwrap();
    assert_eq!(rig_calls(dir.path(), "send").len(), 2);

    send_reply(
        dir.path(),
        json!({"ok":false,"sessionName":SEAT,"sent":false}),
        1,
    );
    assert!(bridge.inbound(&inbound("phone-2", "go on")).await.is_err());
    assert!(!bridge.state.mappings[&key].pending_prompt);
    send_reply(
        dir.path(),
        json!({"ok":true,"sessionName":SEAT,"sent":true}),
        0,
    );
    tick(&mut bridge).await;
    bridge.inbound(&inbound("phone-2", "go on")).await.unwrap();
    assert_eq!(rig_calls(dir.path(), "send").len(), 4);
    assert!(bridge.state.mappings[&key].pending_prompt);
    peer.abort();
}

/// Events recorded from a live `/api/events` stream, renamed to the test seat and renumbered from 101.
fn recorded() -> Vec<String> {
    include_str!("fixtures/openrig-events.jsonl")
        .lines()
        .map(str::to_owned)
        .collect()
}

fn frame(events: &[String]) -> String {
    let mut body = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n".to_owned();
    for event in events {
        match serde_json::from_str::<Value>(event) {
            Ok(value) => body.push_str(&format!("data: {event}\nid: {}\n\n", value["seq"])),
            Err(_) => body.push_str(&format!("data: {event}\n\n")),
        }
    }
    body
}

/// Serves one reply per connection and keeps the last connection open.
async fn event_server(replies: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(vec![]));
    let seen = requests.clone();
    tokio::spawn(async move {
        for (index, reply) in replies.iter().enumerate() {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![];
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte).await.unwrap() == 1 {
                request.push(byte[0]);
            }
            seen.lock()
                .unwrap()
                .push(String::from_utf8(request).unwrap());
            stream.write_all(reply.as_bytes()).await.unwrap();
            if index + 1 == replies.len() {
                std::future::pending::<()>().await;
            }
        }
    });
    (url, requests)
}

async fn follow(config: &BridgeConfig, since: Option<u64>, count: usize) -> Vec<Event> {
    let (tx, mut rx) = mpsc::channel(16);
    let config = config.clone();
    let follower = tokio::spawn(async move { openrig::follow(&config, since, tx).await });
    let mut events = vec![];
    while events.len() < count {
        let next = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv());
        events.push(next.await.unwrap().unwrap());
    }
    follower.abort();
    events
}

fn status(seq: u64, status: &str) -> Event {
    Event {
        seq: Some(seq),
        action: Action::Status {
            seat: SEAT.into(),
            status: status.into(),
        },
    }
}

fn ignored(seq: Option<u64>) -> Event {
    Event {
        seq,
        action: Action::Ignore,
    }
}

const RESYNC: Event = Event {
    seq: None,
    action: Action::Resync,
};

#[tokio::test]
async fn a_stop_hook_on_the_event_stream_forwards_the_reply_without_waiting_for_rig_ps() {
    let (dir, mut config, calls, mut bridge, peer) = connected_seat().await;
    bridge.inbound(&inbound("phone-1", "Fix it")).await.unwrap();
    transcript(
        &config,
        FIRST,
        &[
            ("u0", "user", "Old prompt"),
            ("a0", "final", "Old final"),
            ("u1", "user", "Fix it"),
            ("a1", "final", "Fixed"),
        ],
    );
    let lines = recorded();
    let replay = [
        lines[0].clone(),
        lines[1].clone(),
        "{\"type\":\"agent.activity\",".to_owned(),
        lines[2].clone(),
        lines[3].clone(),
    ];
    let (url, _) = event_server(vec![frame(&replay)]).await;
    config.openrig_url = url;
    let events = follow(&config, Some(100), 6).await;
    assert_eq!(
        events,
        [
            RESYNC,
            ignored(Some(101)),
            ignored(Some(102)),
            ignored(None),
            status(103, "working"),
            status(104, "idle"),
        ]
    );
    let listed = rig_calls(dir.path(), "ps").len();
    for event in events {
        bridge.apply_events(vec![event]).await.unwrap();
    }
    bridge.flush().await.unwrap();
    assert_eq!(sent(&calls, "Fixed"), 1);
    assert_eq!(rig_calls(dir.path(), "ps").len(), listed + 1);
    assert_eq!(bridge.state.openrig_event_seq, Some(104));
    peer.abort();
}

#[tokio::test]
async fn a_dropped_stream_resumes_from_the_last_event_id_across_a_restart() {
    let (dir, mut config, _calls, mut bridge, peer) = connected_seat().await;
    let lines = recorded();
    let (url, requests) = event_server(vec![
        frame(&lines[..1]),
        "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n".into(),
        frame(&lines[3..4]),
    ])
    .await;
    config.openrig_url = url;
    let events = follow(&config, Some(100), 4).await;
    assert_eq!(
        events,
        [RESYNC, ignored(Some(101)), RESYNC, status(104, "idle")]
    );
    let requests = requests.lock().unwrap().clone();
    assert!(requests[0].contains("Last-Event-ID: 100\r\n"));
    assert!(requests[1].contains("Last-Event-ID: 101\r\n"));
    assert!(requests[2].contains("Last-Event-ID: 101\r\n"));
    bridge.apply_events(events).await.unwrap();
    drop(bridge);
    let reopened = Bridge::open(config).unwrap();
    assert_eq!(reopened.state.openrig_event_seq, Some(104));
    drop(dir);
    peer.abort();
}

#[tokio::test]
async fn a_first_start_acts_only_on_events_created_after_it_connects() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(dir.path());
    let lines = recorded();
    let mut fresh: Value = serde_json::from_str(&lines[3]).unwrap();
    fresh["seq"] = json!(200);
    fresh["createdAt"] = json!("9999-12-31 23:59:59");
    let (url, requests) = event_server(vec![frame(&[lines[3].clone(), fresh.to_string()])]).await;
    config.openrig_url = url;
    let events = follow(&config, None, 3).await;
    assert_eq!(events, [RESYNC, ignored(Some(104)), status(200, "idle")]);
    assert!(!requests.lock().unwrap()[0].contains("Last-Event-ID"));
}

#[tokio::test]
async fn a_session_change_event_follows_the_new_session_and_keeps_the_phone_reply() {
    let (dir, mut config, calls, mut bridge, peer) = connected_seat().await;
    bridge
        .inbound(&inbound("phone-1", "Next step"))
        .await
        .unwrap();
    // The seat restarts into a new session before codr reads the prompt in the old one.
    transcript(
        &config,
        SECOND,
        &[("n0", "user", "Next step"), ("n1", "final", "Next final")],
    );
    seats(dir.path(), &[node(SEAT, SECOND, "idle", 2)]);
    let (url, _) = event_server(vec![frame(&recorded()[4..])]).await;
    config.openrig_url = url;
    let events = follow(&config, Some(104), 2).await;
    assert_eq!(
        events,
        [
            RESYNC,
            Event {
                seq: Some(105),
                action: Action::Resync
            }
        ]
    );
    bridge.apply_events(events).await.unwrap();
    bridge.flush().await.unwrap();
    let key = json!(["openrig", SEAT]).to_string();
    let record = bridge.state.mappings[&key].agent_record.as_ref().unwrap();
    assert_eq!(record.agent_session.as_ref().unwrap().value, SECOND);
    assert_eq!(sent(&calls, "Next final"), 1);
    peer.abort();
}

#[test]
fn the_herdr_backend_stays_the_default_and_openrig_needs_seats() {
    let base = format!(
        "phone_npub = {:?}\nwn_agent_socket = \"/tmp/wn.sock\"\naccount_id_hex = {:?}\nstate_path = \"/tmp/state.json\"\n",
        phone_npub(),
        "aa".repeat(32)
    );
    assert!(BridgeConfig::from_toml(&base).is_err());
    let herdr =
        BridgeConfig::from_toml(&format!("{base}herdr_socket = \"/tmp/herdr.sock\"\n")).unwrap();
    assert_eq!(herdr.backend, Backend::Herdr);
    assert!(BridgeConfig::from_toml(&format!("{base}backend = \"openrig\"\n")).is_err());
    let openrig = BridgeConfig::from_toml(&format!(
        "{base}backend = \"openrig\"\nopenrig_seats = [{SEAT:?}]\n"
    ))
    .unwrap();
    assert_eq!(openrig.rig_bin, Path::new("rig"));

    let agent: herdr_white_noise_bridge::herdr::Agent = serde_json::from_value(json!({
        "pane_id":"w1:p1","terminal_id":"term1","agent":"claude","agent_status":"idle",
        "agent_session":{"agent":"claude","source":"herdr:claude","kind":"id","value":FIRST},
        "terminal_title_stripped":null}))
    .unwrap();
    assert_eq!(
        agent.key(),
        json!(["herdr:claude", "claude", "id", FIRST]).to_string()
    );
    assert!(serde_json::to_value(&agent).unwrap().get("seat").is_none());
    assert!(
        agent
            .group_description()
            .ends_with("Managed by codr via HERDR.")
    );
}
