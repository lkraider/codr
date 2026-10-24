use bech32::ToBase32;
use herdr_white_noise_bridge::{
    BridgeState, forwarding,
    state::{self, Mapping},
    wire::read_line,
};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::{io::AsyncWriteExt, net::UnixListener};

const SEAT: &str = "operator-agent@kernel";
const GROUP: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const OTHER: &str = "cccccccccccccccccccccccccccccccc";
const MESSAGE: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

fn write_config(dir: &Path) -> std::path::PathBuf {
    let phone = bech32::encode("npub", [0x11u8; 32].to_base32(), bech32::Variant::Bech32).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        format!(
            "phone_npub = {phone:?}\naccount_id_hex = {:?}\nherdr_socket = {:?}\nwn_agent_socket = {:?}\nstate_path = {:?}\nopencode_bin = {:?}\ncodex_home = {:?}\nclaude_home = {:?}\n",
            "aa".repeat(32),
            dir.join("herdr.sock"),
            dir.join("wn.sock"),
            dir.join("state.json"),
            dir.join("opencode"),
            dir.join("codex"),
            dir.join("claude"),
        ),
    )
    .unwrap();
    path
}

fn write_state(dir: &Path, forwarding_enabled: bool) {
    let mut state = BridgeState {
        forwarding_enabled,
        ..Default::default()
    };
    for (key, group) in [
        (serde_json::to_string(&("openrig", SEAT)).unwrap(), GROUP),
        (
            serde_json::to_string(&("openrig", "dev-build@workshop")).unwrap(),
            OTHER,
        ),
    ] {
        state.mappings.insert(
            key,
            Mapping {
                group_id_hex: Some(group.into()),
                active: true,
                ..Default::default()
            },
        );
    }
    state.save(&dir.join("state.json")).unwrap();
}

async fn mdk(path: &Path, calls: Arc<Mutex<Vec<Value>>>) -> tokio::task::JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(stream);
            let request = read_line(&mut reader).await.unwrap();
            calls.lock().unwrap().push(request.clone());
            assert_eq!(request["type"], "send_final");
            let mut response = json!({"type":"final_sent","message_ids_hex":[MESSAGE]});
            response["id"] = request["id"].clone();
            response["marmot_agent_control"] = request["marmot_agent_control"].clone();
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            reader.get_mut().write_all(&bytes).await.unwrap();
        }
    })
}

async fn notify(config: &Path, args: &[&str]) -> std::process::Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_herdr-white-noise-bridge"))
        .args(["--config", config.to_str().unwrap(), "notify"])
        .args(args)
        .output()
        .await
        .unwrap()
}

#[tokio::test]
async fn notify_reaches_a_seat_group_while_the_worker_holds_the_state_lock() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path());
    write_state(dir.path(), true);
    let _worker = state::lock(&dir.path().join("state.json")).unwrap();
    let before = std::fs::read(dir.path().join("state.json")).unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let server = mdk(&dir.path().join("wn.sock"), calls.clone()).await;

    let output = notify(&config, &["--seat", SEAT, "Merge the branch?"]).await;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let printed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(printed, json!({"group":GROUP,"message_id":MESSAGE}));
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["group_id_hex"], GROUP);
    assert_eq!(calls[0]["text"], "Merge the branch?");
    assert!(
        calls[0]["idempotency_key"]
            .as_str()
            .is_some_and(|key| !key.is_empty())
    );
    assert_eq!(
        std::fs::read(dir.path().join("state.json")).unwrap(),
        before
    );
    server.abort();
}

#[tokio::test]
async fn notify_refuses_an_unknown_seat_and_at_desk_while_the_worker_runs() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path());
    write_state(dir.path(), true);
    let _worker = state::lock(&dir.path().join("state.json")).unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let server = mdk(&dir.path().join("wn.sock"), calls.clone()).await;

    let unknown = notify(&config, &["--seat", "nobody@kernel", "Question"]).await;
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("Unknown seat: nobody@kernel"));

    write_state(dir.path(), false);
    let desk = notify(&config, &["--seat", SEAT, "Question"]).await;
    assert!(!desk.status.success());
    assert!(String::from_utf8_lossy(&desk.stderr).contains("Forwarding is At desk"));

    let both = notify(&config, &["--seat", SEAT, "--group", GROUP, "Question"]).await;
    assert!(!both.status.success());
    assert!(calls.lock().unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn notify_follows_the_forwarding_control_before_the_worker_copies_it() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path());
    write_state(dir.path(), true);
    let _worker = state::lock(&dir.path().join("state.json")).unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let server = mdk(&dir.path().join("wn.sock"), calls.clone()).await;

    forwarding::set(&dir.path().join("state.json"), false).unwrap();
    let desk = notify(&config, &["--seat", SEAT, "Question"]).await;
    assert!(!desk.status.success());
    assert!(String::from_utf8_lossy(&desk.stderr).contains("Forwarding is At desk"));

    forwarding::set(&dir.path().join("state.json"), true).unwrap();
    let resuming = notify(&config, &["--seat", SEAT, "Question"]).await;
    assert!(!resuming.status.success());
    assert!(String::from_utf8_lossy(&resuming.stderr).contains("Forwarding is At desk"));

    assert!(calls.lock().unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn notify_without_a_running_worker_sends_directly_and_prints_the_message_id() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path());
    write_state(dir.path(), true);
    let before = std::fs::read(dir.path().join("state.json")).unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let server = mdk(&dir.path().join("wn.sock"), calls.clone()).await;

    let output = notify(&config, &["Merge the branch?"]).await;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let printed: Vec<Value> = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(printed.len(), 2);
    assert!(printed.contains(&json!({"group":GROUP,"message_id":MESSAGE})));
    assert!(printed.contains(&json!({"group":OTHER,"message_id":MESSAGE})));
    assert_eq!(calls.lock().unwrap().len(), 2);
    assert_eq!(
        std::fs::read(dir.path().join("state.json")).unwrap(),
        before
    );
    server.abort();
}
