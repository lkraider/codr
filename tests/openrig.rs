use bech32::ToBase32;
use herdr_white_noise_bridge::{BridgeConfig, bridge::Bridge, config::Backend, wire::read_line};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::{io::AsyncWriteExt, net::UnixListener};

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
        json!({"ok":false,"sessionName":SEAT,"sent":false,"reason":"guard_unavailable"}),
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
    assert_eq!(rig_calls(dir.path(), "send").len(), 3);
    assert!(bridge.state.mappings[&key].pending_prompt);
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
