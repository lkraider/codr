use crate::{BridgeConfig, wire::Socket};
use anyhow::{Context, Result, anyhow, ensure};
use serde_json::json;
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncWriteExt;

const LIMIT: usize = 4000;

/// Turns Markdown into text for a speech engine and cuts it at `LIMIT` characters.
pub fn spoken_text(markdown: &str, truncated: bool) -> String {
    let mut lines = Vec::new();
    let mut fence: Option<&str> = None;
    let mut table = false;
    for line in markdown.lines() {
        let line = line.trim();
        if let Some(marker) = fence {
            if line.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if line.starts_with("```") || line.starts_with("~~~") {
            fence = Some(&line[..3]);
            lines.push("Code block skipped.".to_owned());
            continue;
        }
        if line.starts_with('|') {
            if !table {
                lines.push("Table skipped.".to_owned());
            }
            table = true;
            continue;
        }
        table = false;
        if line.len() >= 3 && line.chars().all(|c| matches!(c, '-' | '*' | '_' | ' ')) {
            continue;
        }
        let mut text = line.trim_start_matches(['#', '>']).trim_start();
        for marker in ["- ", "* ", "+ "] {
            text = text.strip_prefix(marker).unwrap_or(text);
        }
        let text = inline(text);
        if !text.trim().is_empty() {
            lines.push(text);
        }
    }
    let text = lines.join("\n");
    let over = text.chars().count() > LIMIT;
    if !over && !truncated {
        return text;
    }
    let mut cut: String = text.chars().take(LIMIT).collect();
    if over && let Some(space) = cut.rfind(char::is_whitespace) {
        cut.truncate(space);
    }
    format!("{}\nThe reply is cut here.", cut.trim_end())
}

fn inline(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '!' && rest[1..].starts_with('[') {
            rest = &rest[1..];
            continue;
        }
        if c == '['
            && let Some(close) = rest.find(']')
            && rest[close + 1..].starts_with('(')
            && let Some(end) = rest[close..].find(')')
        {
            out.push_str(&rest[1..close]);
            rest = &rest[close + end + 1..];
            continue;
        }
        if rest.starts_with("__") || rest.starts_with("~~") {
            rest = &rest[2..];
            continue;
        }
        if !matches!(c, '*' | '`') {
            out.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Speaks one bridge message and sends the audio to its group. The audio file is removed afterwards.
pub async fn speak(
    config: &BridgeConfig,
    socket: &Socket,
    group: &str,
    message_id: &str,
) -> Result<()> {
    ensure!(
        !message_id.is_empty() && message_id.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid message ID"
    );
    let found = socket
        .marmot(
            json!({"type":"timeline_message_get","account_id_hex":config.account_id_hex,
        "group_id_hex":group,"message_id_hex":message_id}),
        )
        .await?;
    let row = &found["message"];
    ensure!(
        found["type"] == "timeline_message"
            && row["message_id_hex"] == message_id
            && row["availability"] == "available"
            && row["sender"]["account_id_hex"] == config.account_id_hex,
        "The message to speak is unavailable"
    );
    let text = spoken_text(
        row["text"].as_str().unwrap_or(""),
        row["text_truncated"] == true,
    );
    ensure!(!text.trim().is_empty(), "The message has no text to speak");
    let dir = config.speak_dir.as_ref().context("speak_dir is unset")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let path = dir.join(format!("{message_id}.m4a"));
    let result = match render(config, &text, &path).await {
        Ok(()) => send_audio(config, socket, group, &path).await,
        error => error,
    };
    let _ = std::fs::remove_file(&path);
    result
}

async fn render(config: &BridgeConfig, text: &str, path: &Path) -> Result<()> {
    let command = &config.speak_command;
    ensure!(!command.is_empty(), "speak_command is unset");
    let mut child = tokio::process::Command::new(&command[0])
        .args(&command[1..])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    tokio::time::timeout(Duration::from_secs(300), async {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(text.as_bytes()).await?;
        drop(stdin);
        ensure!(child.wait().await?.success(), "speak_command failed");
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|_| anyhow!("speak_command timed out"))??;
    ensure!(
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0),
        "speak_command wrote no audio"
    );
    Ok(())
}

async fn send_audio(
    config: &BridgeConfig,
    socket: &Socket,
    group: &str,
    path: &Path,
) -> Result<()> {
    let response = socket
        .marmot(
            json!({"type":"send_media","account_id_hex":config.account_id_hex,"group_id_hex":group,
        "attachments":[{"path":path,"media_type":"audio/mp4","file_name":"spoken-reply.m4a"}],
        "caption":null,"idempotency_key":uuid::Uuid::new_v4().to_string()}),
        )
        .await?;
    ensure!(
        response["type"] == "final_sent"
            && response["message_ids_hex"]
                .as_array()
                .is_some_and(|ids| !ids.is_empty()),
        "MDK did not confirm the audio"
    );
    Ok(())
}
