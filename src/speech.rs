use crate::{BridgeConfig, wire::Socket};
use anyhow::{Context, Result, anyhow, ensure};
use serde_json::json;
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::Path,
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::io::AsyncWriteExt;

const LIMIT: usize = 4000;
const NOTHING: &str = "Nothing to speak.";
pub const DEFAULT_ADAPT_PROMPT: &str = "Translate the text below into Brazilian Portuguese and adapt it for speech. \
The audio must sound natural in Brazilian Portuguese, so adapt the translation to help the text-to-speech. \
Apple's enhanced pt-BR voice Luciana speaks it. Adapt the text for that voice. Output only the text to speak.";

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
        let text = without_pictographs(&inline(text));
        if !text.is_empty() {
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

/// Dingbats, U+2B00 arrows, enclosed alphanumerics and U+1F800 arrows count only at their emoji
/// code points, so marks such as ✓ ✗ ❶ ➔ 🄰 stay.
fn pictograph(c: char) -> bool {
    matches!(c,
        '\u{200D}' | '\u{20E3}' | '\u{FE0E}' | '\u{FE0F}'
        | '\u{231A}'..='\u{231B}' | '\u{2328}' | '\u{23CF}' | '\u{23E9}'..='\u{23FA}'
        | '\u{25A0}'..='\u{25FF}' | '\u{2600}'..='\u{26FF}'
        | '\u{2702}' | '\u{2705}' | '\u{2708}'..='\u{270D}' | '\u{270F}' | '\u{2712}'
        | '\u{2714}' | '\u{2716}' | '\u{271D}' | '\u{2721}' | '\u{2728}' | '\u{2733}'
        | '\u{2734}' | '\u{2744}' | '\u{2747}' | '\u{274C}' | '\u{274E}' | '\u{2753}'..='\u{2755}'
        | '\u{2757}' | '\u{2763}' | '\u{2764}' | '\u{2795}'..='\u{2797}' | '\u{27A1}'
        | '\u{27B0}' | '\u{27BF}'
        | '\u{2B05}'..='\u{2B07}' | '\u{2B1B}' | '\u{2B1C}' | '\u{2B50}' | '\u{2B55}'
        | '\u{1F000}'..='\u{1F0FF}'
        | '\u{1F170}' | '\u{1F171}' | '\u{1F17E}' | '\u{1F17F}' | '\u{1F18E}'
        | '\u{1F191}'..='\u{1F19A}' | '\u{1F1E6}'..='\u{1F1FF}'
        | '\u{1F200}'..='\u{1F7FF}' | '\u{1F900}'..='\u{1FAFF}' | '\u{E0020}'..='\u{E007F}')
}

/// Removes emoji. A digit keycap such as `1️⃣` keeps its digit. `#` and `*` keycaps go whole.
fn without_pictographs(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let keycap = |i: usize| {
        chars[i + 1..]
            .iter()
            .find(|&&c| !matches!(c, '\u{FE0E}' | '\u{FE0F}'))
            == Some(&'\u{20E3}')
    };
    let kept: String = chars
        .iter()
        .enumerate()
        .filter(|&(i, &c)| !pictograph(c) && !(matches!(c, '#' | '*') && keycap(i)))
        .map(|(_, &c)| c)
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
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

pub(crate) fn nothing_to_speak(error: &anyhow::Error) -> bool {
    error.to_string() == NOTHING
}

/// Speaks one bridge message. On failure, tells the group directly, because the outbox drops text while At desk.
pub async fn speak_or_report(
    config: &BridgeConfig,
    socket: &Socket,
    group: &str,
    message_id: &str,
    prompt_unreadable: &AtomicBool,
) -> Result<()> {
    let result = speak(config, socket, group, message_id, prompt_unreadable).await;
    if let Err(error) = &result {
        let text = if nothing_to_speak(error) {
            NOTHING
        } else {
            "Speech failed. The bridge log has the reason."
        };
        notice(config, socket, group, text).await;
    }
    result
}

async fn notice(config: &BridgeConfig, socket: &Socket, group: &str, text: &str) {
    let notice = socket
        .marmot(
            json!({"type":"send_final","account_id_hex":config.account_id_hex,"group_id_hex":group,
            "text":text,"reply_to_message_id_hex":null,
            "idempotency_key":uuid::Uuid::new_v4().to_string()}),
        )
        .await;
    if let Err(error) = notice.and_then(|response| {
        ensure!(
            response["type"] == "final_sent",
            "MDK did not confirm the notice"
        );
        Ok(())
    }) {
        eprintln!("Speech notice failed: {error:#}");
    }
}

/// Speaks one bridge message and sends the audio to its group. The audio file is removed afterwards.
async fn speak(
    config: &BridgeConfig,
    socket: &Socket,
    group: &str,
    message_id: &str,
    prompt_unreadable: &AtomicBool,
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
    ensure!(!text.is_empty(), NOTHING);
    let dir = config.speak_dir.as_ref().context("speak_dir is unset")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let text = if config.speak_adapt_command.is_empty() {
        text
    } else {
        let (prompt, log) =
            adapt_prompt(config.speak_adapt_prompt_file.as_deref(), prompt_unreadable);
        if log {
            eprintln!("speak_adapt_prompt_file is missing or empty. The built-in prompt is used.");
        }
        match adapt(config, &prompt, &text, dir).await {
            Ok(adapted) => adapted,
            Err(error) => {
                eprintln!("Speech translation failed: {error:#}");
                notice(
                    config,
                    socket,
                    group,
                    "Translation failed. The original text is spoken.",
                )
                .await;
                text
            }
        }
    };
    let path = tempfile::Builder::new()
        .prefix("spoken-")
        .suffix(".m4a")
        .tempfile_in(dir)?
        .into_temp_path();
    let result = match render(config, &text, &path).await {
        Ok(()) => send_audio(config, socket, group, message_id, &path).await,
        error => error,
    };
    let _ = path.close();
    result
}

/// Reads the prompt file, or gives `DEFAULT_ADAPT_PROMPT` when it is missing or empty.
/// The flag asks the caller to log. It is true on the first unreadable read after a readable one.
pub fn adapt_prompt(path: Option<&Path>, unreadable: &AtomicBool) -> (String, bool) {
    let Some(path) = path else {
        return (DEFAULT_ADAPT_PROMPT.to_owned(), false);
    };
    match std::fs::read_to_string(path) {
        Ok(prompt) if !prompt.trim().is_empty() => {
            unreadable.store(false, Ordering::Relaxed);
            (prompt.trim().to_owned(), false)
        }
        _ => (
            DEFAULT_ADAPT_PROMPT.to_owned(),
            !unreadable.swap(true, Ordering::Relaxed),
        ),
    }
}

/// Runs `speak_adapt_command` in `dir`. Errors carry no command output, because it holds message text.
async fn adapt(config: &BridgeConfig, prompt: &str, text: &str, dir: &Path) -> Result<String> {
    let command = &config.speak_adapt_command;
    let prompt = format!("{prompt}\n\n{text}");
    let mut child = tokio::process::Command::new(&command[0])
        .args(&command[1..])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let output = tokio::time::timeout(Duration::from_secs(120), async {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(prompt.as_bytes()).await?;
        drop(stdin);
        let output = child.wait_with_output().await?;
        ensure!(
            output.status.success(),
            "speak_adapt_command failed: {}",
            output.status
        );
        Ok::<_, anyhow::Error>(output.stdout)
    })
    .await
    .map_err(|_| anyhow!("speak_adapt_command timed out"))??;
    let adapted = spoken_text(&String::from_utf8_lossy(&output), false);
    ensure!(!adapted.is_empty(), "speak_adapt_command printed no text");
    Ok(adapted)
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
    message_id: &str,
    path: &Path,
) -> Result<()> {
    let response = socket
        .marmot(
            json!({"type":"send_media","account_id_hex":config.account_id_hex,"group_id_hex":group,
        "attachments":[{"path":path,"media_type":"audio/mp4","file_name":"spoken-reply.m4a"}],
        "caption":null,"reply_to_message_id_hex":message_id,"idempotency_key":uuid::Uuid::new_v4().to_string()}),
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
