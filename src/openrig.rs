use crate::{
    BridgeConfig,
    herdr::{Agent, Session},
    wire::NotSubmitted,
};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::mpsc::Sender,
    time::timeout,
};

/// OpenRig typed nothing and gave this reason, such as `target_needs_input` or `typing_guard_enabled`.
#[derive(Debug, thiserror::Error)]
#[error("OpenRig refused the prompt: {0}")]
pub struct Refused(pub String);

/// The seat's tmux pane, from the `tmuxAttachCommand` that `rig ps` reports.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pane {
    /// `-L name` or `-S path`, empty for the default server.
    pub server: Vec<String>,
    pub target: String,
}

impl Pane {
    pub fn parse(attach: &str) -> Option<Self> {
        let mut words = attach.split_whitespace();
        if words.next()? != "tmux" {
            return None;
        }
        let (mut server, mut target) = (vec![], None);
        while let Some(word) = words.next() {
            match word {
                "-L" | "-S" => server.extend([word.to_owned(), words.next()?.to_owned()]),
                "-t" => target = Some(words.next()?.to_owned()),
                _ => {}
            }
        }
        Some(Self {
            server,
            target: target?,
        })
    }

    async fn tmux(&self, config: &BridgeConfig, args: &[&str]) -> Result<String> {
        let output = timeout(
            Duration::from_secs(5),
            Command::new(&config.tmux_bin)
                .args(&self.server)
                .args(args)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        ensure!(output.status.success(), "tmux failed: {}", output.status);
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    pub async fn in_copy_mode(&self, config: &BridgeConfig) -> Result<bool> {
        // tmux 3.8 prints an empty line and exits 0 for a target that does not exist.
        match self
            .tmux(
                config,
                &[
                    "display-message",
                    "-p",
                    "-t",
                    &self.target,
                    "#{pane_in_mode}",
                ],
            )
            .await?
            .as_str()
        {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => bail!("tmux did not report the pane mode"),
        }
    }

    pub async fn leave_copy_mode(&self, config: &BridgeConfig) -> Result<()> {
        self.tmux(config, &["send-keys", "-t", &self.target, "-X", "cancel"])
            .await
            .map(drop)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// The SSE id, which OpenRig sets to the event's `seq`.
    pub seq: Option<u64>,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Ignore,
    /// A Claude Code hook moved a listed seat to this `rig ps` agent status.
    Status {
        seat: String,
        status: String,
        /// The hook was `Stop`, so the turn's final reply is due.
        stop: bool,
    },
    /// Seats or sessions changed, or events were missed. Read `rig ps` again.
    Resync,
}

pub async fn snapshot(config: &BridgeConfig) -> Result<Vec<Agent>> {
    let output = timeout(
        Duration::from_secs(20),
        Command::new(&config.rig_bin)
            .args(["ps", "--nodes", "-A", "--json", "--full"])
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    ensure!(output.status.success(), "OpenRig did not list its seats");
    let nodes: Vec<Value> = serde_json::from_slice(&output.stdout)?;
    Ok(nodes
        .iter()
        .filter(|node| {
            node["sessionStatus"] == "running"
                && node["canonicalSessionName"]
                    .as_str()
                    .is_some_and(|name| config.openrig_seats.iter().any(|seat| seat == name))
        })
        .map(agent)
        .collect())
}

fn agent(node: &Value) -> Agent {
    let seat = node["canonicalSessionName"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let claude = node["runtime"] == "claude-code";
    let session = node["resumeToken"]
        .as_str()
        .filter(|token| claude && node["resumeType"] == "claude_id" && !token.is_empty())
        .map(|token| Session {
            source: "openrig".into(),
            agent: "claude".into(),
            kind: "id".into(),
            value: token.into(),
        });
    let decided = &node["activityState"];
    let status = if decided["needsInput"]["count"].as_u64().unwrap_or(0) > 0 {
        "blocked"
    } else {
        node["agentActivity"]["state"]
            .as_str()
            .and_then(status)
            .or(match decided["display"].as_str() {
                Some("working") => Some("working"),
                Some("idle") => Some("idle"),
                _ => None,
            })
            .unwrap_or("unknown")
    };
    Agent {
        pane_id: seat.clone(),
        terminal_id: seat.clone(),
        tab_id: seat.clone(),
        tab_label: Some(seat.clone()),
        workspace_id: node["rigName"].as_str().unwrap_or_default().to_owned(),
        workspace_label: None,
        agent: if claude {
            "claude".into()
        } else {
            node["runtime"].as_str().unwrap_or_default().to_owned()
        },
        agent_status: status.into(),
        agent_session: session,
        state_change_seq: decided["seq"].as_u64().unwrap_or(0),
        cwd: node["cwd"].as_str().unwrap_or_default().to_owned(),
        foreground_cwd: None,
        terminal_title_stripped: Some(seat.clone()),
        seat: Some(seat),
        pane: node["tmuxAttachCommand"].as_str().and_then(Pane::parse),
    }
}

fn status(state: &str) -> Option<&'static str> {
    match state {
        "needs_input" => Some("blocked"),
        "running" => Some("working"),
        "idle" => Some("idle"),
        _ => None,
    }
}

pub async fn get(config: &BridgeConfig, agent: &Agent) -> Result<Agent> {
    let current = snapshot(config)
        .await?
        .into_iter()
        .find(|current| current.seat == agent.seat)
        .ok_or_else(|| anyhow::anyhow!("OpenRig seat ended"))?;
    ensure!(
        current.agent_session == agent.agent_session,
        "OpenRig seat changed its session"
    );
    Ok(current)
}

pub async fn prompt(config: &BridgeConfig, agent: &Agent, text: &str) -> Result<()> {
    let seat = agent
        .seat
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Agent has no OpenRig seat"))?;
    let child = Command::new(&config.rig_bin)
        .args(["send", seat, "--raw", "--json", "--", text])
        .kill_on_drop(true)
        .output();
    let output = match timeout(Duration::from_secs(60), child).await {
        Err(_) => bail!("OpenRig send timed out"),
        Ok(Err(_)) => return Err(NotSubmitted.into()),
        Ok(Ok(output)) => output,
    };
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
    // A retained send has sent:false, but OpenRig can still deliver it later.
    let retained = reply["outcome"] == "retained";
    if output.status.success() && reply["ok"] == true && reply["sent"] != false && !retained {
        return Ok(());
    }
    let reason = reply["reason"].as_str().filter(|reason| !reason.is_empty());
    if let Some(reason) = reason
        .filter(|reason| *reason == "target_needs_input" || (reply["sent"] == false && !retained))
    {
        return Err(Refused(reason.into()).into());
    }
    if reply["sent"] == false && !retained {
        return Err(NotSubmitted.into());
    }
    bail!("OpenRig did not confirm the prompt")
}

pub fn classify(event: &Value, seats: &[String]) -> Action {
    let kind = event["type"].as_str().unwrap_or_default();
    let topology = ["session.", "node.", "rig."]
        .iter()
        .any(|prefix| kind.starts_with(prefix));
    let Some(seat) = event["sessionName"].as_str() else {
        return if topology {
            Action::Resync
        } else {
            Action::Ignore
        };
    };
    if !seats.iter().any(|listed| listed == seat) {
        return Action::Ignore;
    }
    if kind == "agent.activity" {
        return match event["activity"]["state"].as_str().and_then(status) {
            Some(status) => Action::Status {
                seat: seat.into(),
                status: status.into(),
                stop: event["activity"]["rawEvent"] == "Stop",
            },
            None => Action::Resync,
        };
    }
    if topology || kind == "agent.session_identity" {
        Action::Resync
    } else {
        Action::Ignore
    }
}

/// Follows `GET <openrig_url>/api/events` until the receiver closes.
/// Each connection starts with a resync and ignores events created before it connected.
/// Without a stored sequence, OpenRig replays its whole history.
pub async fn follow(config: &BridgeConfig, mut since: Option<u64>, tx: Sender<Event>) {
    let url = format!("{}/api/events", config.openrig_url.trim_end_matches('/'));
    let mut down = false;
    let mut wait = 1;
    loop {
        let skip_before = daemon_now();
        let mut curl = Command::new("curl");
        curl.args(["-sN", "--fail", "-H", "Accept: text/event-stream"]);
        if let Some(seq) = since {
            curl.arg("-H").arg(format!("Last-Event-ID: {seq}"));
        }
        let child = curl
            .arg(&url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn();
        if let Ok(mut child) = child
            && let Some(stdout) = child.stdout.take()
        {
            let mut lines = BufReader::new(stdout).lines();
            let mut connected = false;
            let mut data = String::new();
            let mut id = None;
            while let Ok(Some(line)) = lines.next_line().await {
                if !connected {
                    connected = true;
                    wait = 1;
                    if down {
                        eprintln!("OpenRig event stream reconnected.");
                        down = false;
                    }
                    if tx
                        .send(Event {
                            seq: None,
                            action: Action::Resync,
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                if let Some(value) = line.strip_prefix("data:") {
                    data.push_str(value.strip_prefix(' ').unwrap_or(value));
                    continue;
                }
                if let Some(value) = line.strip_prefix("id:") {
                    id = value.trim().parse().ok();
                    continue;
                }
                if !line.is_empty() || data.is_empty() {
                    continue;
                }
                let action = match serde_json::from_str::<Value>(&data) {
                    Ok(event) => {
                        id = id.or(event["seq"].as_u64());
                        let old = skip_before.as_deref().is_some_and(|start| {
                            event["createdAt"].as_str().is_some_and(|at| at < start)
                        });
                        if old {
                            Action::Ignore
                        } else {
                            classify(&event, &config.openrig_seats)
                        }
                    }
                    Err(_) => {
                        eprintln!("Skipped an OpenRig event that is not JSON.");
                        Action::Ignore
                    }
                };
                data.clear();
                let seq = id.take();
                since = seq.or(since);
                if tx.send(Event { seq, action }).await.is_err() {
                    return;
                }
            }
            let _ = child.wait().await;
        }
        if !down {
            eprintln!("OpenRig event stream is unavailable; codr resyncs with rig ps every 60 s.");
            down = true;
        }
        tokio::time::sleep(Duration::from_secs(wait)).await;
        wait = (wait * 2).min(30);
    }
}

/// The daemon stamps `createdAt` with SQLite's `datetime('now')`, so SQLite gives the same format.
fn daemon_now() -> Option<String> {
    rusqlite::Connection::open_in_memory()
        .and_then(|db| db.query_row("SELECT datetime('now')", [], |row| row.get(0)))
        .ok()
}
