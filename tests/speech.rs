use bech32::ToBase32;
use herdr_white_noise_bridge::{
    BridgeConfig, BridgeState, bridge::Bridge, speech::spoken_text, wire::read_line,
};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{io::AsyncWriteExt, net::UnixListener};

const BRIDGE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PHONE: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const GROUP: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MESSAGE: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const REPLY: &str = "## Result\n\nThe **build** passed. See [the log](https://example.com/log).\n\n```sh\ncargo test\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- `fmt` is clean";

#[derive(Clone, Default)]
struct Calls {
    requests: Arc<Mutex<Vec<Value>>>,
    sent_audio: Arc<Mutex<Vec<Vec<u8>>>>,
}

fn speaker(dir: &Path, exit: u8) -> PathBuf {
    let path = dir.join("speak.sh");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\ncat > {:?}\nprintf 'm4a audio' > \"$1\"\nexit {exit}\n",
            dir.join("spoken.txt")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn config(dir: &Path, exit: u8) -> BridgeConfig {
    let phone = bech32::encode("npub", [0x11u8; 32].to_base32(), bech32::Variant::Bech32).unwrap();
    serde_json::from_value(json!({"phone_npub":phone,"account_id_hex":BRIDGE,
        "herdr_socket":dir.join("herdr.sock"),"wn_agent_socket":dir.join("wn.sock"),"state_path":dir.join("state.json"),
        "speak_command":[speaker(dir, exit)],"speak_reaction":"😮","speak_dir":dir.join("speech")}))
    .unwrap()
}

async fn connector(path: &Path, calls: Calls, refuse_media: bool) -> tokio::task::JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(stream);
            let request = read_line(&mut reader).await.unwrap();
            calls.requests.lock().unwrap().push(request.clone());
            let mut response = match request["type"].as_str().unwrap() {
                "timeline_message_get" => json!({"type":"timeline_message","account_id_hex":BRIDGE,
                    "group_id_hex":GROUP,"message_id_hex":MESSAGE,"message":{"message_id_hex":MESSAGE,
                    "availability":"available","kind":9,"sender":{"account_id_hex":BRIDGE},
                    "text":REPLY,"text_truncated":false}}),
                "send_media" if refuse_media => {
                    json!({"type":"error","error":{"code":"media_upload_disabled"}})
                }
                "send_media" => {
                    let path = request["attachments"][0]["path"].as_str().unwrap();
                    calls
                        .sent_audio
                        .lock()
                        .unwrap()
                        .push(std::fs::read(path).unwrap());
                    json!({"type":"final_sent","message_ids_hex":["dd".repeat(32)]})
                }
                "send_final" => json!({"type":"final_sent","message_ids_hex":["ef".repeat(32)]}),
                other => panic!("unexpected request {other}"),
            };
            response["id"] = request["id"].clone();
            response["marmot_agent_control"] = request["marmot_agent_control"].clone();
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            reader.get_mut().write_all(&bytes).await.unwrap();
        }
    })
}

fn reaction(actor: &str, emoji: &str, target_sender: &str) -> Value {
    json!({"marmot_agent_control":"marmot.agent-control.v2","type":"reaction_added","account_id_hex":BRIDGE,
        "group_id_hex":GROUP,"event_id_hex":"ee".repeat(32),"target_message_id_hex":MESSAGE,
        "actor":{"account_id_hex":actor,"is_self":false},"emoji":emoji,"recorded_at":1,
        "target":{"message_id_hex":MESSAGE,"availability":"available","sender":{"account_id_hex":target_sender}}})
}

async fn bridge(dir: &Path, exit: u8) -> Bridge {
    BridgeState::with_mapping("[\"herdr\",\"w1:p1\"]", GROUP.into())
        .save(&dir.join("state.json"))
        .unwrap();
    Bridge::open(config(dir, exit)).unwrap()
}

fn notices(calls: &Calls) -> Vec<Value> {
    let requests = calls.requests.lock().unwrap();
    requests
        .iter()
        .filter(|r| r["type"] == "send_final")
        .cloned()
        .collect()
}

async fn settle(bridge: &mut Bridge, calls: &Calls) {
    for _ in 0..200 {
        bridge.process_speech().await.unwrap();
        if !bridge.state.spoken_messages.is_empty() || !notices(calls).is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("speech did not finish");
}

#[tokio::test]
async fn a_phone_reaction_sends_the_spoken_bridge_message_once_and_removes_the_audio() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Calls::default();
    let server = connector(&dir.path().join("wn.sock"), calls.clone(), false).await;
    let mut bridge = bridge(dir.path(), 0).await;

    bridge
        .inbound(&reaction(PHONE, "😮", BRIDGE))
        .await
        .unwrap();
    bridge
        .inbound(&reaction(PHONE, "😮", BRIDGE))
        .await
        .unwrap();
    settle(&mut bridge, &calls).await;

    let requests = calls.requests.lock().unwrap().clone();
    let types: Vec<_> = requests.iter().map(|r| r["type"].clone()).collect();
    assert_eq!(types, ["timeline_message_get", "send_media"]);
    let attachment = &requests[1]["attachments"][0];
    assert_eq!(requests[1]["group_id_hex"], GROUP);
    assert_eq!(attachment["media_type"], "audio/mp4");
    let path = Path::new(attachment["path"].as_str().unwrap());
    assert_eq!(
        path,
        dir.path().join("speech").join(format!("{MESSAGE}.m4a"))
    );
    assert_eq!(calls.sent_audio.lock().unwrap()[0], b"m4a audio");
    assert!(!path.exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("spoken.txt")).unwrap(),
        "Result\nThe build passed. See the log.\nCode block skipped.\nTable skipped.\nfmt is clean"
    );
    assert_eq!(bridge.state.spoken_messages, [MESSAGE]);
    assert!(notices(&calls).is_empty());

    bridge
        .inbound(&reaction(PHONE, "😮", BRIDGE))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    bridge.process_speech().await.unwrap();
    assert_eq!(calls.requests.lock().unwrap().len(), 2);
    server.abort();
}

#[tokio::test]
async fn reactions_from_another_sender_with_another_emoji_or_on_a_phone_message_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Calls::default();
    let server = connector(&dir.path().join("wn.sock"), calls.clone(), false).await;
    let mut bridge = bridge(dir.path(), 0).await;

    for event in [
        reaction(&"22".repeat(32), "😮", BRIDGE),
        reaction(PHONE, "👍", BRIDGE),
        reaction(PHONE, "😮", PHONE),
    ] {
        bridge.inbound(&event).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    bridge.process_speech().await.unwrap();

    assert!(calls.requests.lock().unwrap().is_empty());
    assert!(bridge.state.spoken_messages.is_empty());
    server.abort();
}

#[tokio::test]
async fn a_failed_speech_command_tells_the_phone_at_desk_and_removes_the_audio() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Calls::default();
    let server = connector(&dir.path().join("wn.sock"), calls.clone(), false).await;
    let mut bridge = bridge(dir.path(), 1).await;
    bridge.state.forwarding_enabled = false;

    bridge
        .inbound(&reaction(PHONE, "😮", BRIDGE))
        .await
        .unwrap();
    settle(&mut bridge, &calls).await;

    assert_eq!(calls.requests.lock().unwrap().len(), 2);
    let notice = &notices(&calls)[0];
    assert_eq!(notice["group_id_hex"], GROUP);
    assert_eq!(
        notice["text"],
        "Speech failed. The bridge log has the reason."
    );
    assert!(bridge.state.outbox.is_empty());
    assert!(bridge.state.spoken_messages.is_empty());
    assert_eq!(
        std::fs::read_dir(dir.path().join("speech"))
            .unwrap()
            .count(),
        0
    );
    server.abort();
}

#[tokio::test]
async fn a_refused_media_send_tells_the_phone_and_removes_the_audio() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Calls::default();
    let server = connector(&dir.path().join("wn.sock"), calls.clone(), true).await;
    let mut bridge = bridge(dir.path(), 0).await;

    bridge
        .inbound(&reaction(PHONE, "😮", BRIDGE))
        .await
        .unwrap();
    settle(&mut bridge, &calls).await;

    assert_eq!(calls.requests.lock().unwrap().len(), 3);
    assert_eq!(
        notices(&calls)[0]["text"],
        "Speech failed. The bridge log has the reason."
    );
    assert!(bridge.state.spoken_messages.is_empty());
    assert_eq!(
        std::fs::read_dir(dir.path().join("speech"))
            .unwrap()
            .count(),
        0
    );
    server.abort();
}

#[test]
fn long_text_is_cut_at_a_word_with_a_spoken_note() {
    let text = spoken_text(&"word ".repeat(1000), false);
    assert!(text.ends_with("word\nThe reply is cut here."));
    assert!(text.chars().count() <= 4000 + "\nThe reply is cut here.".len());
    assert_eq!(spoken_text("Short.", false), "Short.");
    assert_eq!(
        spoken_text("Short.", true),
        "Short.\nThe reply is cut here."
    );
}
