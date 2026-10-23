use crate::{
    BridgeConfig,
    herdr::{Agent, Session},
    wire::NotSubmitted,
};
use anyhow::{Result, bail, ensure};
use serde_json::Value;
use std::time::Duration;
use tokio::{process::Command, time::timeout};

#[derive(Debug, thiserror::Error)]
#[error("OpenRig refused input while the agent waits on an interactive prompt")]
pub struct NeedsInput;

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
    let reported = &node["agentActivity"]["state"];
    let decided = &node["activityState"];
    let status =
        if reported == "needs_input" || decided["needsInput"]["count"].as_u64().unwrap_or(0) > 0 {
            "blocked"
        } else {
            match reported.as_str() {
                Some("running") => "working",
                Some("idle") => "idle",
                _ => match decided["display"].as_str() {
                    Some("working") => "working",
                    Some("idle") => "idle",
                    _ => "unknown",
                },
            }
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
    if reply["reason"] == "target_needs_input" {
        return Err(NeedsInput.into());
    }
    if reply["sent"] == false && !retained {
        return Err(NotSubmitted.into());
    }
    bail!("OpenRig did not confirm the prompt")
}
