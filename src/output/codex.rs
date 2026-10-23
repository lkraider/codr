use super::{FinalMessage, native};
use anyhow::{Result, ensure};
use std::{collections::HashSet, path::Path};

pub fn read(home: &Path, session: &str) -> Result<Vec<FinalMessage>> {
    snapshot(home, session).map(|(history, _)| history.finals)
}

pub(super) fn snapshot(
    home: &Path,
    session: &str,
) -> Result<(super::SessionHistory, Option<super::tasks::TaskList>)> {
    let session = native::session_id(session)?;
    let suffix = format!("-{session}.jsonl");
    let path = native::find(
        &[
            (home.join("sessions"), 3),
            (home.join("archived_sessions"), 3),
        ],
        |name| name.starts_with("rollout-") && name.ends_with(&suffix),
    )?;
    let mut verified = false;
    let mut seen = HashSet::new();
    let mut replies = Vec::new();
    let mut tasks = super::tasks::Tracker::default();
    let mut latest_turn = None;
    let mut turns = std::collections::BTreeSet::new();
    let mut has_started_turn = false;
    let mut user_inputs = Vec::new();
    native::records(&path, |row| {
        tasks.codex_record(&row);
        let payload = &row["payload"];
        if row["type"] == "event_msg" && payload["type"] == "user_message" {
            let text = payload["message"].as_str().unwrap_or("");
            let id = format!(
                "codex:user:{}:{}",
                user_inputs.len(),
                row["timestamp"].as_str().unwrap_or("")
            );
            user_inputs
                .push(crate::activity::UserInput::new(id, text).in_turn(latest_turn.clone()));
        }
        if ((row["type"] == "event_msg"
            && matches!(
                payload["type"].as_str(),
                Some("task_started" | "turn_started" | "task_complete" | "turn_complete")
            ))
            || row["type"] == "turn_context")
            && let Some(turn) = payload["turn_id"].as_str()
        {
            let turn = format!("codex:{turn}");
            turns.insert(turn.clone());
            if row["type"] == "turn_context"
                || matches!(
                    payload["type"].as_str(),
                    Some("task_started" | "turn_started")
                )
            {
                has_started_turn = true;
                latest_turn = Some(turn);
            } else if !has_started_turn {
                latest_turn = Some(turn);
            }
        }
        if row["type"] == "session_meta" {
            ensure!(
                payload["id"]
                    .as_str()
                    .and_then(|id| native::session_id(id).ok())
                    .as_deref()
                    == Some(&session),
                "Codex transcript session identity mismatch"
            );
            verified = true;
        }
        if row["type"] != "event_msg"
            || !matches!(
                payload["type"].as_str(),
                Some("task_complete" | "turn_complete")
            )
            || !payload["error"].is_null()
        {
            return Ok(());
        }
        let Some(text) = payload["last_agent_message"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
        else {
            return Ok(());
        };
        let turn = payload["turn_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Codex completion lacks its turn ID"))?;
        let id = format!("codex:{turn}");
        if seen.insert(id.clone()) {
            replies.push(FinalMessage {
                turn: Some(id.clone()),
                id,
                text: text.into(),
            });
        }
        Ok(())
    })?;
    ensure!(
        verified,
        "Codex transcript has no verified session metadata"
    );
    Ok((
        super::SessionHistory {
            finals: replies,
            latest_turn,
            turns: turns.into_iter().collect(),
            user_inputs,
        },
        tasks.latest,
    ))
}
