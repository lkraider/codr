use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::{
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
    process::Stdio,
    sync::atomic::{AtomicU8, Ordering},
    time::Duration,
};
use tokio::process::Command;

pub mod claude;
pub mod codex;
pub mod decision;
mod native;
pub mod opencode;
pub mod tasks;

pub fn supported(agent: &str) -> bool {
    matches!(agent, "opencode" | "codex" | "claude")
}

pub async fn read_session(
    config: &crate::config::BridgeConfig,
    session: &crate::herdr::Session,
) -> Result<Vec<FinalMessage>> {
    Ok(read_history(config, session).await?.finals)
}

#[derive(Clone, Debug, Default)]
pub struct SessionHistory {
    pub finals: Vec<FinalMessage>,
    pub latest_turn: Option<String>,
    pub turns: Vec<String>,
    pub user_inputs: Vec<crate::activity::UserInput>,
}

pub async fn read_history(
    config: &crate::config::BridgeConfig,
    session: &crate::herdr::Session,
) -> Result<SessionHistory> {
    ensure!(
        session.kind == "id",
        "Unsupported native session reference kind"
    );
    match session.agent.as_str() {
        "opencode" if config.opencode_db.is_some() => {
            Ok(read_snapshot(config, session).await?.history)
        }
        "opencode" => Ok(select_history(
            export_data(&config.opencode_bin, &session.value).await?,
        )),
        "codex" | "claude" => {
            let provider = session.agent.clone();
            let id = session.value.clone();
            let home = if provider == "codex" {
                config.codex_home.clone()
            } else {
                config.claude_home.clone()
            };
            tokio::task::spawn_blocking(move || {
                if provider == "codex" {
                    codex::snapshot(&home, &id).map(|(history, _)| history)
                } else {
                    claude::snapshot(&home, &id).map(|(history, _)| history)
                }
            })
            .await
            .map_err(|_| anyhow::anyhow!("Native transcript reader failed"))?
        }
        _ => Ok(SessionHistory::default()),
    }
}

#[derive(Clone, Debug)]
pub struct FinalMessage {
    pub id: String,
    pub text: String,
    pub turn: Option<String>,
}

// Unknown fields include tool outputs, attachments and token accounting. Serde skips
// them directly from the reader instead of retaining the complete export in memory.
#[derive(Deserialize)]
struct Export {
    messages: Vec<Message>,
}
#[derive(Clone, Deserialize)]
struct Message {
    info: Info,
    parts: Vec<Part>,
}
#[derive(Clone, Deserialize)]
struct Info {
    #[serde(default)]
    id: String,
    role: String,
    #[serde(default)]
    time: MessageTime,
    finish: Option<String>,
    error: Option<Value>,
    #[serde(default)]
    summary: Value,
    #[serde(default, rename = "parentID")]
    parent_id: Option<String>,
}
#[derive(Clone, Default, Deserialize)]
struct MessageTime {
    completed: Option<u64>,
}
#[derive(Clone, Deserialize)]
struct Part {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
    #[serde(default)]
    synthetic: bool,
    #[serde(default)]
    ignored: bool,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    state: Value,
}

pub fn final_messages_from_reader(reader: impl Read) -> Result<Vec<FinalMessage>> {
    select_finals(serde_json::from_reader(reader)?)
}

pub fn final_messages(export: &Value) -> Result<Vec<FinalMessage>> {
    select_finals(serde_json::from_value(export.clone())?)
}

pub fn final_messages_v2(export: &Value) -> Result<Vec<FinalMessage>> {
    select_finals(from_v2(serde_json::from_value(export.clone())?))
}

fn select_finals(export: Export) -> Result<Vec<FinalMessage>> {
    Ok(select_history(export).finals)
}

fn select_history(export: Export) -> SessionHistory {
    history_from_data(&export)
}

fn history_from_data(export: &Export) -> SessionHistory {
    let mut out = vec![];
    let mut latest_turn = None;
    let mut turns = std::collections::BTreeSet::new();
    let mut has_user = false;
    let mut user_inputs = Vec::new();
    for message in &export.messages {
        let info = &message.info;
        if info.role == "user" {
            let parts: Vec<_> = message
                .parts
                .iter()
                .filter(|part| part.kind == "text" && !part.synthetic && !part.ignored)
                .collect();
            if !info.id.is_empty()
                && info.summary != true
                && (message.parts.is_empty()
                    || !parts.is_empty()
                    || message
                        .parts
                        .iter()
                        .any(|part| matches!(part.kind.as_str(), "file" | "image")))
            {
                let text = parts
                    .iter()
                    .filter_map(|part| part.text.as_deref())
                    .collect::<Vec<_>>()
                    .join("\n");
                let id = format!("opencode:{}", info.id);
                user_inputs.push(if message.parts.is_empty() {
                    crate::activity::UserInput::pending(id)
                } else {
                    crate::activity::UserInput::new(id.clone(), &text).in_turn(Some(id))
                });
            }
            latest_turn = Some(format!("opencode:{}", info.id));
            turns.insert(latest_turn.clone().unwrap());
            has_user = true;
            continue;
        }
        let turn = info
            .parent_id
            .as_ref()
            .map(|id| format!("opencode:{id}"))
            .or_else(|| latest_turn.clone());
        if let Some(turn) = &turn {
            turns.insert(turn.clone());
        }
        if !has_user && turn.is_some() {
            latest_turn = turn.clone();
        }
        if info.role != "assistant"
            || info.time.completed.is_none()
            || info.finish.as_deref() != Some("stop")
            || info.error.is_some()
            || info.summary == true
        {
            continue;
        }
        let text = message
            .parts
            .iter()
            .filter(|p| p.kind == "text" && !p.synthetic && !p.ignored)
            .filter_map(|p| p.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        if !text.trim().is_empty() {
            out.push(FinalMessage {
                id: info.id.clone(),
                text,
                turn,
            });
        }
    }
    SessionHistory {
        finals: out,
        latest_turn,
        turns: turns.into_iter().collect(),
        user_inputs,
    }
}

pub async fn export(bin: &Path, session: &str) -> Result<Vec<FinalMessage>> {
    select_finals(export_data(bin, session).await?)
}

const EXPORT_UNKNOWN: u8 = 0;
const EXPORT_V1: u8 = 1;
const EXPORT_V2: u8 = 2;

/// The first successful command selects the API for later reads. OpenCode 2.x removed
/// the v1 `--pure export` command, so one failed attempt is expected after an upgrade.
static EXPORT_API: AtomicU8 = AtomicU8::new(EXPORT_UNKNOWN);

async fn export_data(bin: &Path, session: &str) -> Result<Export> {
    let attempts = if EXPORT_API.load(Ordering::Relaxed) == EXPORT_V2 {
        [EXPORT_V2, EXPORT_V1]
    } else {
        [EXPORT_V1, EXPORT_V2]
    };
    let mut failures = [String::new(), String::new()];
    for api in attempts {
        match run_export(bin, session, api).await {
            Ok(export) => {
                EXPORT_API.store(api, Ordering::Relaxed);
                return Ok(export);
            }
            Err(error) => failures[usize::from(api - EXPORT_V1)] = format!("{error:#}"),
        }
    }
    anyhow::bail!(
        "OpenCode export failed (v1: {}; v2: {})",
        failures[0],
        failures[1]
    )
}

async fn run_export(bin: &Path, session: &str, api: u8) -> Result<Export> {
    // Regular files avoid OpenCode's observed stdout-pipe truncation. They are anonymous and owner-only.
    let mut output = tempfile::tempfile()?;
    let mut errors = tempfile::tempfile()?;
    let args: &[&str] = if api == EXPORT_V2 {
        &["session", "export", session]
    } else {
        &["--pure", "export", session]
    };
    let mut command = Command::new(bin);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(errors.try_clone()?)
        .kill_on_drop(true);
    let status = tokio::time::timeout(Duration::from_secs(30), command.status()).await??;
    if !status.success() {
        errors.seek(SeekFrom::Start(0))?;
        let mut message = String::new();
        errors.take(4096).read_to_string(&mut message)?;
        let message = message.trim();
        anyhow::bail!(
            "exited with {}: {}",
            status,
            if message.is_empty() {
                "no error output"
            } else {
                message
            }
        );
    }
    ensure!(
        output.metadata()?.len() <= 128 * 1024 * 1024,
        "OpenCode export exceeds 128 MiB"
    );
    output.seek(SeekFrom::Start(0))?;
    match api {
        EXPORT_V2 => tokio::task::spawn_blocking(move || {
            Ok(from_v2(serde_json::from_reader(BufReader::new(output))?))
        })
        .await
        .map_err(|_| anyhow::anyhow!("OpenCode export reader failed"))?,
        _ => tokio::task::spawn_blocking(move || {
            Ok(serde_json::from_reader(BufReader::new(output))?)
        })
        .await
        .map_err(|_| anyhow::anyhow!("OpenCode export reader failed"))?,
    }
}

#[derive(Deserialize)]
struct ExportV2 {
    messages: Vec<MessageV2>,
}

#[derive(Deserialize)]
struct MessageV2 {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    content: Vec<PartV2>,
    #[serde(default)]
    time: MessageTimeV2,
    #[serde(default)]
    finish: Option<String>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Default, Deserialize)]
struct MessageTimeV2 {
    #[serde(default)]
    completed: Option<u64>,
}

#[derive(Deserialize)]
struct PartV2 {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    state: Value,
}

/// OpenCode 2.x exports flattened session events instead of v1 messages with parts.
fn from_v2(export: ExportV2) -> Export {
    let messages = export
        .messages
        .into_iter()
        .filter_map(|message| {
            let (role, parts, finish, completed) = match message.kind.as_str() {
                "user" => (
                    "user",
                    vec![Part {
                        kind: "text".into(),
                        text: message.text,
                        synthetic: false,
                        ignored: false,
                        id: None,
                        tool: None,
                        state: Value::Null,
                    }],
                    None,
                    None,
                ),
                "assistant" => (
                    "assistant",
                    message.content.into_iter().map(part_from_v2).collect(),
                    message.finish,
                    message.time.completed,
                ),
                // Subagent markers and idle events are not conversation messages.
                _ => return None,
            };
            Some(Message {
                info: Info {
                    id: message.id,
                    role: role.into(),
                    time: MessageTime { completed },
                    finish,
                    error: message.error,
                    summary: Value::Bool(false),
                    parent_id: None,
                },
                parts,
            })
        })
        .collect();
    Export { messages }
}

fn part_from_v2(part: PartV2) -> Part {
    Part {
        kind: part.kind,
        text: part.text,
        synthetic: false,
        ignored: false,
        id: part.id,
        tool: part.name,
        state: part.state,
    }
}

#[derive(Clone, Debug, Default)]
pub struct SessionSnapshot {
    pub history: SessionHistory,
    pub tasks: Option<tasks::TaskList>,
    pub decision: Option<decision::Decision>,
}

/// Fixed diagnostics never include transcript content, IDs, paths, or CLI stderr.
pub fn read_error_message(error: &anyhow::Error) -> &'static str {
    if let Some(rusqlite::Error::SqliteFailure(code, _)) = error.downcast_ref::<rusqlite::Error>() {
        return match code.code {
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                "database is busy or locked"
            }
            rusqlite::ErrorCode::CannotOpen | rusqlite::ErrorCode::PermissionDenied => {
                "database access failed"
            }
            rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase => {
                "database is corrupt or invalid"
            }
            _ => "database read failed",
        };
    }
    "transcript read failed; verify CLI export support and transcript contents"
}

pub(crate) async fn read_opencode_snapshot(
    config: &crate::BridgeConfig,
    session: &crate::herdr::Session,
    database: std::sync::Arc<std::sync::Mutex<opencode::Reader>>,
) -> Result<SessionSnapshot> {
    if let Some(path) = &config.opencode_db {
        let path = path.clone();
        let id = session.value.clone();
        let read = tokio::task::spawn_blocking(move || {
            database
                .lock()
                .map_err(|_| anyhow::anyhow!("OpenCode database reader failed"))?
                .read_compatible(&path, &id)
        })
        .await
        .map_err(|_| anyhow::anyhow!("OpenCode database reader failed"))??;
        if let Some(read) = read {
            return Ok(read.snapshot);
        }
    }
    let export = export_data(&config.opencode_bin, &session.value).await?;
    Ok(SessionSnapshot {
        history: history_from_data(&export),
        tasks: tasks::from_data(&export),
        decision: decision::from_data(&export).ok().flatten(),
    })
}

/// Read reply, task and decision projections from one consistent transcript.
pub async fn read_snapshot(
    config: &crate::BridgeConfig,
    session: &crate::herdr::Session,
) -> Result<SessionSnapshot> {
    ensure!(
        session.kind == "id",
        "Unsupported native session reference kind"
    );
    match session.agent.as_str() {
        "opencode" => read_opencode_snapshot(config, session, Default::default()).await,
        "codex" | "claude" => {
            let provider = session.agent.clone();
            let id = session.value.clone();
            let home = if provider == "codex" {
                config.codex_home.clone()
            } else {
                config.claude_home.clone()
            };
            tokio::task::spawn_blocking(move || {
                let (history, tasks) = if provider == "codex" {
                    codex::snapshot(&home, &id)?
                } else {
                    claude::snapshot(&home, &id)?
                };
                Ok(SessionSnapshot {
                    history,
                    tasks,
                    decision: None,
                })
            })
            .await
            .map_err(|_| anyhow::anyhow!("Native transcript reader failed"))?
        }
        _ => Ok(SessionSnapshot::default()),
    }
}
