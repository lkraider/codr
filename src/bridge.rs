use crate::{
    BridgeConfig, BridgeState,
    config::{Backend, phone_hex},
    forwarding,
    herdr::{self, Agent},
    openrig, output,
    state::Mapping,
    wire::{PROTOCOL, Socket, read_line},
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub struct Bridge {
    pub config: BridgeConfig,
    pub state: BridgeState,
    pub agents: BTreeMap<String, Agent>,
    pub herdr: Socket,
    pub marmot: Socket,
    pub herdr_available: bool,
    phone: String,
    media_job: Option<tokio::task::JoinHandle<Result<crate::media::Prepared>>>,
    speech_jobs: Vec<SpeechJob>,
    reads: crate::reads::Reads,
    last_snapshot: Vec<Agent>,
    stop_rereads: BTreeMap<String, StopReread>,
}

struct SpeechJob {
    message_id: String,
    job: tokio::task::JoinHandle<Result<()>>,
}

const SPOKEN_LIMIT: usize = 500;

/// A `Stop` hook can arrive before the transcript holds the final reply.
struct StopReread {
    finals: usize,
    started: tokio::time::Instant,
    next: tokio::time::Instant,
    wait: Duration,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        if let Some(job) = self.media_job.take() {
            job.abort();
        }
        for speech in &self.speech_jobs {
            speech.job.abort();
        }
    }
}

impl Bridge {
    pub fn open(config: BridgeConfig) -> Result<Self> {
        let mut state = BridgeState::load(&config.state_path)?;
        let phone = phone_hex(&config.phone_npub)?;
        ensure!(
            state
                .account
                .as_ref()
                .is_none_or(|a| a == &config.account_id_hex)
                && state.phone.as_ref().is_none_or(|p| p == &phone),
            "State belongs to another account or phone"
        );
        state.account = Some(config.account_id_hex.clone());
        state.phone = Some(phone.clone());
        state.save(&config.state_path)?;
        let mut bridge = Self {
            herdr: Socket {
                path: config.herdr_socket.clone(),
            },
            marmot: Socket {
                path: config.wn_agent_socket.clone(),
            },
            config,
            state,
            agents: BTreeMap::new(),
            herdr_available: false,
            phone,
            media_job: None,
            speech_jobs: vec![],
            reads: crate::reads::Reads::default(),
            last_snapshot: vec![],
            stop_rereads: BTreeMap::new(),
        };
        bridge.refresh_forwarding()?;
        Ok(bridge)
    }

    pub fn save(&self) -> Result<()> {
        self.state.save(&self.config.state_path)
    }

    fn backend_name(&self) -> &'static str {
        match self.config.backend {
            Backend::Herdr => "HERDR",
            Backend::OpenRig => "OpenRig",
        }
    }

    pub async fn snapshot(&self) -> Result<Vec<Agent>> {
        match self.config.backend {
            Backend::Herdr => herdr::snapshot(&self.herdr).await,
            Backend::OpenRig => openrig::snapshot(&self.config).await,
        }
    }

    async fn current_agent(&self, agent: &Agent) -> Result<Agent> {
        match self.config.backend {
            Backend::Herdr => herdr::get(&self.herdr, agent).await,
            Backend::OpenRig => openrig::get(&self.config, agent).await,
        }
    }

    async fn prompt_agent(&self, agent: &Agent, text: &str) -> Result<()> {
        match self.config.backend {
            Backend::Herdr => herdr::prompt(&self.herdr, agent, text).await.map(drop),
            Backend::OpenRig => openrig::prompt(&self.config, agent, text).await,
        }
    }

    fn refresh_forwarding(&mut self) -> Result<()> {
        let control = forwarding::load(&self.config.state_path)?;
        if self.state.forwarding_revision.as_deref() != Some(&control.revision) {
            // Preserve the old outbox on the first upgrade, unless a mode was already selected.
            let reset = self.state.forwarding_revision.is_some()
                || control.revision != "initial"
                || !control.enabled;
            if reset {
                self.state.outbox.clear();
                for mapping in self.state.mappings.values_mut() {
                    mapping.response_id = None;
                    mapping.task_revision = None;
                    mapping.forwarding_baseline_pending = true;
                    mapping.blocked_context_seq = None;
                    mapping.decision_checked_at = 0;
                    mapping.decision_refresh_pending = true;
                    if let Some(decision) = mapping.decision.as_mut() {
                        decision.announced = false;
                    }
                }
            }
            self.state.forwarding_enabled = control.enabled && !reset;
            self.state.forwarding_resume_pending = control.enabled && reset;
            self.state.forwarding_revision = Some(control.revision);
            self.save()?;
        }
        Ok(())
    }

    async fn activate_forwarding(&mut self) -> Result<()> {
        self.refresh_forwarding()?;
        if !self.state.forwarding_resume_pending || !self.herdr_available {
            return Ok(());
        }
        let revision = self.state.forwarding_revision.clone();
        let keys: Vec<_> = self
            .state
            .mappings
            .iter()
            .filter(|(key, mapping)| {
                mapping.forwarding_baseline_pending
                    && (mapping.active || self.agents.contains_key(*key))
            })
            .map(|(key, _)| key.clone())
            .collect();
        let mut agents = Vec::new();
        for key in keys {
            let agent = self.agents.get(&key).cloned().or_else(|| {
                self.state.mappings[&key]
                    .agent_record
                    .clone()
                    .map(|mut agent| {
                        agent.agent_status = "done".into();
                        agent
                    })
            });
            agents.push((
                key,
                agent.ok_or_else(|| {
                    anyhow::anyhow!(
                        "Active session identity is unavailable for its forwarding checkpoint"
                    )
                })?,
            ));
        }
        if self.reads.background {
            // Start every checkpoint before waiting for any one session.
            let mut histories = Vec::new();
            let mut pending = false;
            for (key, agent) in &agents {
                match self.read_history(agent).await {
                    Ok(history) => histories.push((key.clone(), agent.clone(), history)),
                    Err(_) => pending = true,
                }
            }
            for (key, agent, history) in histories {
                self.apply_forwarding_baseline(&key, &agent, Some(history))?;
                self.reads.finish(&key);
            }
            if pending {
                return Err(crate::reads::Pending.into());
            }
            self.refresh_forwarding()?;
            if self.state.forwarding_revision == revision {
                self.state.forwarding_enabled = true;
                self.state.forwarding_resume_pending = false;
                self.save()?;
            }
            return Ok(());
        }
        let limit = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
        let mut readers = tokio::task::JoinSet::new();
        for (key, agent) in agents {
            let config = self.config.clone();
            let limit = limit.clone();
            let database = self.reads.database.clone();
            readers.spawn(async move {
                let _permit = limit.acquire_owned().await?;
                let history = if let Some(session) = agent
                    .agent_session
                    .as_ref()
                    .filter(|session| output::supported(&session.agent))
                {
                    Some(
                        crate::reads::snapshot(&config, session, database)
                            .await?
                            .history,
                    )
                } else {
                    None
                };
                Ok::<_, anyhow::Error>((key, agent, history))
            });
        }
        let mut failure = None;
        let mut cancelled = false;
        while let Some(result) = readers.join_next().await {
            // Native spawn_blocking readers cannot be aborted. Drain every reader before
            // retrying, so an error or mode change cannot start another four on top.
            if failure.is_some() || cancelled {
                continue;
            }
            let applied = result
                .map_err(|_| anyhow::anyhow!("Forwarding checkpoint task failed"))
                .and_then(|result| result)
                .and_then(|(key, agent, history)| {
                    self.refresh_forwarding()?;
                    if self.state.forwarding_revision != revision {
                        return Ok(false);
                    }
                    self.apply_forwarding_baseline(&key, &agent, history)?;
                    Ok(true)
                });
            match applied {
                Ok(true) => {}
                Ok(false) => cancelled = true,
                Err(error) => failure = Some(error),
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        if cancelled {
            return Ok(());
        }
        // A newer Desk selection wins over a slow checkpoint or a recovered phone event.
        self.refresh_forwarding()?;
        if self.state.forwarding_resume_pending && self.state.forwarding_revision == revision {
            self.state.forwarding_enabled = true;
            self.state.forwarding_resume_pending = false;
            self.save()?;
        }
        Ok(())
    }

    async fn baseline_forwarding(&mut self, key: &str, agent: &Agent) -> Result<bool> {
        if !self.state.mappings[key].forwarding_baseline_pending {
            return Ok(false);
        }
        let history = if agent
            .agent_session
            .as_ref()
            .is_some_and(|session| output::supported(&session.agent))
        {
            Some(self.read_history(agent).await?)
        } else {
            None
        };
        self.apply_forwarding_baseline(key, agent, history)?;
        Ok(true)
    }

    fn apply_forwarding_baseline(
        &mut self,
        key: &str,
        agent: &Agent,
        history: Option<output::SessionHistory>,
    ) -> Result<()> {
        if let Some(history) = history {
            let mapping = self.state.mappings.get_mut(key).unwrap();
            mapping.activity.baseline(&history.user_inputs);
            let known: HashSet<_> = mapping.final_ids.iter().cloned().collect();
            mapping.final_ids.extend(
                history
                    .finals
                    .into_iter()
                    .map(|reply| reply.id)
                    .filter(|id| !known.contains(id)),
            );
            let muted: HashSet<_> = mapping.muted_turns.iter().cloned().collect();
            mapping
                .muted_turns
                .extend(history.turns.into_iter().filter(|turn| {
                    (agent.settled() || Some(turn) != history.latest_turn.as_ref())
                        && !muted.contains(turn)
                }));
            mapping.output_initialised = true;
            mapping.last_export_at = now();
        }
        let mapping = self.state.mappings.get_mut(key).unwrap();
        mapping.forwarding_baseline_pending = false;
        if agent.settled() {
            mapping.pending_prompt = false;
            mapping.completion_pending = false;
        }
        self.save()?;
        Ok(())
    }

    pub async fn flush(&mut self) -> Result<()> {
        self.refresh_forwarding()?;
        if !self.state.forwarding_enabled || self.activity_unresolved() {
            return Ok(());
        }
        let mut failed = HashSet::new();
        let mut outbox = self.state.outbox.clone();
        // Durable replies and their continuations precede best-effort progress.
        outbox.sort_by_key(|item| item.response.is_some());
        for item in outbox {
            self.refresh_forwarding()?;
            if !self.state.forwarding_enabled || self.activity_unresolved() {
                return Ok(());
            }
            if !self
                .state
                .outbox
                .iter()
                .any(|pending| pending.id == item.id)
            {
                continue;
            }
            if failed.contains(&item.group) {
                continue;
            }
            if let Some(delivery) = &item.response {
                if delivery.finished && delivery.target.is_none() && !delivery.durable {
                    self.state.outbox.retain(|m| m.id != item.id);
                    self.save()?;
                    continue;
                }
                if delivery.sent_text.as_deref() == Some(&item.text) {
                    if delivery.finished {
                        self.state.outbox.retain(|m| m.id != item.id);
                        self.save()?;
                    }
                    continue;
                }
                // Persist the attempt before I/O. Never emit competing edits in one second,
                // including after restart or a response lost after publication.
                if now() < delivery.last_attempt_at.saturating_add(2) {
                    continue;
                }
                let pending = self
                    .state
                    .outbox
                    .iter_mut()
                    .find(|m| m.id == item.id)
                    .unwrap()
                    .response
                    .as_mut()
                    .unwrap();
                pending.last_attempt_at = now();
                let initial = pending
                    .initial_text
                    .get_or_insert_with(|| item.text.clone())
                    .clone();
                self.save()?;
                let request = if let Some(target) = &delivery.target {
                    json!({"type":"edit_message","account_id_hex":self.config.account_id_hex,"group_id_hex":item.group,"target_message_id_hex":target,"text":item.text})
                } else {
                    json!({"type":"send_final","account_id_hex":self.config.account_id_hex,"group_id_hex":item.group,"text":initial,"reply_to_message_id_hex":null,"idempotency_key":item.id})
                };
                let result =
                    tokio::time::timeout(Duration::from_secs(3), self.marmot.marmot(request))
                        .await
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("Progress update timed out")));
                self.state
                    .outbox
                    .iter_mut()
                    .find(|m| m.id == item.id)
                    .unwrap()
                    .response
                    .as_mut()
                    .unwrap()
                    .last_attempt_at = now();
                self.save()?;
                match result {
                    Ok(response)
                        if response["type"] == "final_sent"
                            && response["message_ids_hex"].as_array().is_some_and(|ids| {
                                !ids.is_empty()
                                    && ids
                                        .iter()
                                        .all(|id| id.as_str().is_some_and(|id| !id.is_empty()))
                            }) =>
                    {
                        let sent_text = if delivery.target.is_some() {
                            item.text.clone()
                        } else {
                            initial
                        };
                        let pending = self
                            .state
                            .outbox
                            .iter_mut()
                            .find(|m| m.id == item.id)
                            .unwrap()
                            .response
                            .as_mut()
                            .unwrap();
                        if pending.target.is_none() {
                            pending.target =
                                response["message_ids_hex"][0].as_str().map(str::to_owned);
                        }
                        pending.sent_text = Some(sent_text.clone());
                        if delivery.finished && sent_text == item.text {
                            self.state.outbox.retain(|m| m.id != item.id);
                        }
                        self.save()?;
                    }
                    _ => {
                        if delivery.finished && !delivery.durable {
                            // A failed terminal progress edit must not hold durable answers.
                            self.state.outbox.retain(|m| m.id != item.id);
                            self.save()?;
                        } else {
                            failed.insert(item.group);
                        }
                    }
                }
                continue;
            }
            match self.marmot.marmot(json!({"type":"send_final","account_id_hex":self.config.account_id_hex,"group_id_hex":item.group,"text":item.text,"reply_to_message_id_hex":null,"idempotency_key":item.id})).await {
                Ok(response) if response["type"] == "final_sent" && response["message_ids_hex"].as_array().is_some_and(|ids|!ids.is_empty()) => {
                    self.state.outbox.retain(|m| m.id != item.id); self.save()?;
                }
                _ => { failed.insert(item.group); }
            }
        }
        ensure!(failed.is_empty(), "Some group deliveries remain pending");
        Ok(())
    }

    pub async fn reconcile(&mut self, agents: Vec<Agent>) -> Result<()> {
        self.refresh_forwarding()?;
        self.last_snapshot = agents.clone();
        let mut current = BTreeMap::new();
        let mut uncertain_terminals = HashSet::new();
        for mut agent in agents {
            // Never attach a group to a location that a replacement session can reuse.
            let Some(session) = agent.agent_session.as_ref() else {
                uncertain_terminals.insert(agent.terminal_id.clone());
                continue;
            };
            if agent.agent.is_empty() {
                agent.agent = session.agent.clone();
                agent.agent_status = "unknown".into();
            }
            ensure!(
                current.insert(agent.key(), agent).is_none(),
                "Two live agents report the same session identity"
            );
        }
        self.agents = current;
        for key in self.agents.keys() {
            self.reads.set_ended(key, false);
        }
        let read_keys = self
            .agents
            .keys()
            .cloned()
            .chain(
                self.state
                    .mappings
                    .iter()
                    .filter(|(_, mapping)| mapping.active)
                    .map(|(key, _)| key.clone()),
            )
            .collect();
        self.reads.retain(&read_keys);
        self.herdr_available = true;
        self.activate_forwarding().await?;
        ensure!(
            !self.state.forwarding_resume_pending,
            "Forwarding resume awaits its current history checkpoint"
        );
        let keys: Vec<_> = self.agents.keys().cloned().collect();
        for key in keys {
            let agent = self.agents[&key].clone();
            if let Err(error) = self.reconcile_agent(&key, &agent).await {
                if error.downcast_ref::<crate::reads::Pending>().is_none() {
                    eprintln!("A session could not reconcile; other sessions remain available.");
                }
            } else {
                self.reads.finish(&key);
            }
        }
        let ended: Vec<_> = self
            .state
            .mappings
            .iter()
            .filter(|(key, m)| {
                m.active
                    && !self.agents.contains_key(*key)
                    && !m
                        .agent_record
                        .as_ref()
                        .is_some_and(|a| uncertain_terminals.contains(&a.terminal_id))
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in ended {
            self.reads.set_ended(&key, true);
            let baselining = self.state.mappings[&key].forwarding_baseline_pending;
            if let Some(group) = self.state.mappings[&key].group_id_hex.clone() {
                if let Some(agent) = self.state.mappings[&key].agent_record.clone()
                    && self.collect_finals(&key, &agent, &group).await.is_err()
                {
                    eprintln!("Final reply for an ended session remains pending.");
                    continue;
                }
                self.reads.finish(&key);
                if self.config.edit_responses && self.state.mappings[&key].response_id.is_some() {
                    self.state.update_response(
                        &key,
                        &group,
                        "> ended No completed reply was recorded.",
                        true,
                    );
                }
                if !baselining {
                    self.state.enqueue(&group, "> ended This group retains its history. A restored native session will reconnect here.");
                }
            }
            if self.config.auto_forwarding && self.state.mappings[&key].activity.unresolved() {
                forwarding::set(&self.config.state_path, false)?;
                self.refresh_forwarding()?;
                self.state.mappings.get_mut(&key).unwrap().activity.retire();
            }
            let m = self.state.mappings.get_mut(&key).unwrap();
            m.active = false;
            m.pending_prompt = false;
            m.completion_pending = false;
        }
        self.save()
    }

    async fn seed_native_history(&mut self, key: &str, agent: &Agent) -> Result<bool> {
        if !self.state.mappings[key].output_initialised
            && let Some(_) = agent
                .agent_session
                .as_ref()
                .filter(|session| matches!(session.agent.as_str(), "codex" | "claude"))
        {
            let history = self.read_history(agent).await?;
            let mapping = self.state.mappings.get_mut(key).unwrap();
            mapping.activity.baseline(&history.user_inputs);
            // A new session's transcript can already hold the reply to a phone prompt sent before the change.
            let phone_turn = |turn: &Option<String>| {
                turn.as_ref()
                    .is_some_and(|turn| mapping.activity.phone_turn(turn))
            };
            let seen: Vec<_> = history
                .finals
                .into_iter()
                .filter(|reply| !phone_turn(&reply.turn))
                .map(|reply| reply.id)
                .collect();
            mapping.final_ids.extend(seen);
            mapping.output_initialised = true;
            mapping.last_export_at = now();
            self.save()?;
            return Ok(true);
        }
        Ok(false)
    }

    async fn collect_finals(&mut self, key: &str, agent: &Agent, group: &str) -> Result<()> {
        if self.baseline_forwarding(key, agent).await? {
            return Ok(());
        }
        // The ended-session path also reaches this method without live reconciliation.
        if self.seed_native_history(key, agent).await? {
            return Ok(());
        }
        if agent
            .agent_session
            .as_ref()
            .is_some_and(|s| output::supported(&s.agent))
        {
            let history = self.read_history(agent).await?;
            self.observe_activity(key, &history)?;
            if self.config.phone_replies_only {
                let answered = history.latest_turn.as_ref().is_some_and(|latest| {
                    history
                        .finals
                        .iter()
                        .any(|reply| reply.turn.as_ref() == Some(latest))
                });
                let activity = &mut self.state.mappings.get_mut(key).unwrap().activity;
                activity.retain_turns(&history.turns);
                activity.track_latest(history.latest_turn.as_deref(), answered);
            }
            if !self.state.forwarding_enabled {
                let mapping = self.state.mappings.get_mut(key).unwrap();
                let muted: HashSet<_> = mapping.muted_turns.iter().cloned().collect();
                mapping.muted_turns.extend(
                    history
                        .turns
                        .into_iter()
                        .filter(|turn| !muted.contains(turn)),
                );
            }
            let known: HashSet<_> = self.state.mappings[key].final_ids.iter().cloned().collect();
            for reply in history.finals {
                if !known.contains(&reply.id) {
                    let mapping = &self.state.mappings[key];
                    if reply
                        .turn
                        .as_ref()
                        .is_none_or(|turn| !mapping.muted_turns.contains(turn))
                        && (!self.config.phone_replies_only
                            || reply
                                .turn
                                .as_ref()
                                .is_some_and(|turn| mapping.activity.phone_turn(turn)))
                    {
                        if self.config.edit_responses {
                            self.state.complete_progress(key);
                        }
                        self.state.enqueue(group, &reply.text);
                    }
                    self.state
                        .mappings
                        .get_mut(key)
                        .unwrap()
                        .final_ids
                        .push(reply.id);
                }
            }
            self.state.mappings.get_mut(key).unwrap().last_export_at = now();
        }
        Ok(())
    }

    async fn reconcile_agent(&mut self, key: &str, agent: &Agent) -> Result<()> {
        if let Some(mapping) = self.state.mappings.get_mut(key) {
            let expired = mapping.activity.expire_held(now());
            if let Some(group) = mapping.group_id_hex.clone().filter(|_| expired > 0) {
                for _ in 0..expired {
                    self.state.enqueue(&group, "Stopped waiting for a phone prompt sent more than 30 minutes ago. Its reply will not be forwarded.");
                }
                self.save()?;
            }
        }
        if self.reads.background
            && agent
                .agent_session
                .as_ref()
                .is_some_and(|s| output::supported(&s.agent))
        {
            let needs_read = self.state.mappings.get(key).is_none_or(|mapping| {
                self.config.auto_forwarding
                    || !mapping.output_initialised
                    || mapping.forwarding_baseline_pending
                    || (self.config.edit_responses && agent.agent_status == "working")
                    || (agent.agent_status == "blocked"
                        && agent
                            .agent_session
                            .as_ref()
                            .is_some_and(|s| s.agent == "opencode")
                        && (mapping.blocked_context_seq != Some(agent.state_change_seq)
                            || now().saturating_sub(mapping.decision_checked_at) >= 5))
                    || (agent.settled()
                        && (mapping.last_state_change_seq != agent.state_change_seq
                            || mapping.last_agent_status != agent.agent_status
                            || mapping.completion_pending
                            || now().saturating_sub(mapping.last_export_at)
                                >= self.config.settled_reread_secs))
            });
            if needs_read {
                let result = self
                    .reads
                    .read(&self.config, agent, self.state.forwarding_revision.clone())
                    .await;
                if let Err(error) = result {
                    // Pending reads are silent. Progress/blocked-state updates remain
                    // available when optional task or decision extraction fails.
                    if !matches!(agent.agent_status.as_str(), "blocked" | "working")
                        || error.downcast_ref::<crate::reads::Pending>().is_some()
                    {
                        return Err(error);
                    }
                }
            }
        }
        if !self.state.mappings.contains_key(key) {
            // Capture the initial final-message watermark before exposing a new group.
            let mut m = Mapping {
                joined_at: now(),
                active: true,
                agent_record: Some(agent.clone()),
                ..Mapping::default()
            };
            if agent
                .agent_session
                .as_ref()
                .is_some_and(|s| output::supported(&s.agent))
            {
                let history = self.read_history(agent).await?;
                m.activity.baseline(&history.user_inputs);
                m.final_ids = history.finals.into_iter().map(|f| f.id).collect();
                m.output_initialised = true;
                if self.config.edit_responses && agent.settled() {
                    m.task_revision = self
                        .read_tasks(agent)
                        .await
                        .ok()
                        .flatten()
                        .map(|tasks| tasks.revision);
                }
            }
            self.state.mappings.insert(key.to_owned(), m);
            self.save()?;
        }
        if self.state.mappings[key]
            .agent_record
            .as_ref()
            .is_some_and(|record| record.agent_session != agent.agent_session)
        {
            // A new session on the same seat brings a transcript with its own history.
            self.state.mappings.get_mut(key).unwrap().output_initialised = false;
        }
        self.baseline_forwarding(key, agent).await?;
        // Older bridge versions created these groups without a transcript watermark.
        // Seed their existing history once when enabling native reply support.
        self.seed_native_history(key, agent).await?;
        if self.config.auto_forwarding
            && agent
                .agent_session
                .as_ref()
                .is_some_and(|session| output::supported(&session.agent))
        {
            let history = self.read_history(agent).await?;
            self.observe_activity(key, &history)?;
        }
        self.refresh_forwarding()?;
        let name = agent.group_name();
        let description = agent.group_description();
        let m = &self.state.mappings[key];
        if m.group_id_hex.is_none()
            && !m.creating
            && self.state.forwarding_enabled
            && !self.activity_unresolved()
        {
            let capabilities = self
                .marmot
                .marmot(json!({"type":"group_create_capabilities"}))
                .await?;
            ensure!(
                capabilities["type"] == "group_create_capabilities"
                    && capabilities["initial_admins"] == true,
                "The connector must support founding owner-admin rights before creating groups"
            );
            self.refresh_forwarding()?;
            if !self.state.forwarding_enabled {
                return Ok(());
            }
            self.state.mappings.get_mut(key).unwrap().creating = true;
            self.save()?;
            // No blind retries: GroupCreate has no idempotency key, even after a timeout.
            match self.marmot.marmot(json!({"type":"group_create_with_admins", "account_id_hex":self.config.account_id_hex,"name":name,"members":[self.config.phone_npub],"admins":[self.config.phone_npub],"description":description})).await {
                    Ok(response) => {
                        ensure!(response["type"] == "group_created", "Unexpected group creation response");
                        let group = response["group_id_hex"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| anyhow::anyhow!("Missing group ID"))?.to_owned();
                        let m = self.state.mappings.get_mut(key).unwrap(); m.group_id_hex = Some(group.clone()); m.creating = false;
                        m.group_name = Some(name.clone()); m.group_description = Some(description.clone());
                        self.state.enqueue(&group, &format!("Connected to {}: {}. Send a message here to prompt this session. Use /status for its state.", self.backend_name(), agent.title()));
                        self.save()?;
                    }
                    Err(error) => {
                        if error.downcast_ref::<crate::wire::NotSubmitted>().is_some() {
                            self.state.mappings.get_mut(key).unwrap().creating = false; self.save()?;
                        } else { eprintln!("Group creation needs inspection. Use status and resolve-create before retrying."); }
                        return Ok(());
                    }
                }
        }
        let Some(group) = self.state.mappings[key].group_id_hex.clone() else {
            return Ok(());
        };
        self.sync_group_profile(key, agent, &group).await;
        let previous = &self.state.mappings[key];
        let changed = previous.last_agent_status != agent.agent_status
            || previous.last_state_change_seq != agent.state_change_seq;
        let status_changed = previous.last_agent_status != agent.agent_status;
        let resumed = previous.last_agent_status == "blocked";
        let previous_settled = matches!(previous.last_agent_status.as_str(), "idle" | "done");
        let quiet = self.config.phone_replies_only && !previous.activity.awaiting_phone();
        if self.config.edit_responses {
            if agent.agent_status == "working" && !quiet {
                if previous_settled
                    && !self.state.mappings[key].pending_prompt
                    && self.state.mappings[key].response_id.is_some()
                {
                    self.state.update_response(
                        key,
                        &group,
                        "> idle No completed reply was recorded.",
                        true,
                    );
                }
                self.state.begin_response(key, &group);
                if resumed {
                    self.state.update_response(key, &group, "> working", false);
                }
            } else if status_changed
                && !agent.settled()
                && self.state.mappings[key].response_id.is_some()
            {
                self.state.update_response(
                    key,
                    &group,
                    &format!("> {}", agent.agent_status),
                    false,
                );
            }
        } else if previous.last_agent_status != agent.agent_status
            && agent.agent_status != "done"
            && (!self.config.phone_replies_only
                || agent.agent_status == "blocked"
                || resumed && !quiet && agent.agent_status == "working")
        {
            self.state
                .enqueue(&group, &format!("> {}", agent.agent_status));
        }
        let m = self.state.mappings.get_mut(key).unwrap();
        m.last_agent_status = agent.agent_status.clone();
        m.last_state_change_seq = agent.state_change_seq;
        m.active = true;
        m.agent_record = Some(agent.clone());
        if changed && agent.settled() {
            m.completion_pending = true;
        }
        self.save()?;
        if self.config.edit_responses
            && !quiet
            && self.state.forwarding_enabled
            && agent.agent_status == "working"
            && agent.agent_session.is_some()
        {
            match self.read_tasks(agent).await {
                Ok(Some(tasks))
                    if self.state.mappings[key].task_revision.as_ref() != Some(&tasks.revision)
                        || resumed =>
                {
                    self.state
                        .update_response(key, &group, &tasks.text(), false);
                    self.state.mappings.get_mut(key).unwrap().task_revision = Some(tasks.revision);
                }
                Ok(_) => {}
                Err(_) => eprintln!("Task list unavailable; response delivery remains active."),
            }
        }
        let opencode = agent
            .agent_session
            .as_ref()
            .is_some_and(|session| session.agent == "opencode");
        if self.state.forwarding_enabled && agent.agent_status == "blocked" && opencode {
            let mapping = &self.state.mappings[key];
            if mapping.blocked_context_seq != Some(agent.state_change_seq)
                || (mapping
                    .decision
                    .as_ref()
                    .is_none_or(|decision| !decision.announced)
                    || mapping.decision_refresh_pending)
                    && now().saturating_sub(mapping.decision_checked_at) >= 5
            {
                let decision = self
                    .reads
                    .read(&self.config, agent, self.state.forwarding_revision.clone())
                    .await
                    .map(|snapshot| snapshot.decision.clone());
                self.state
                    .mappings
                    .get_mut(key)
                    .unwrap()
                    .decision_checked_at = now();
                self.state
                    .mappings
                    .get_mut(key)
                    .unwrap()
                    .decision_refresh_pending = !matches!(&decision, Ok(Some(_)));
                match decision {
                    Ok(Some(decision)) => {
                        self.announce_decision(key, &group, decision)?;
                    }
                    _ if self.state.mappings[key].blocked_context_seq
                        != Some(agent.state_change_seq) =>
                    {
                        let notice = "> blocked Agent needs input in the terminal. No supported decision is available yet.";
                        if self.config.edit_responses {
                            self.state.update_response(key, &group, notice, false);
                        } else {
                            self.state.enqueue(&group, notice);
                        }
                    }
                    _ => {}
                }
                self.state
                    .mappings
                    .get_mut(key)
                    .unwrap()
                    .blocked_context_seq = Some(agent.state_change_seq);
            }
        } else if self.config.backend == Backend::Herdr
            && agent.agent_status == "blocked"
            && self.state.mappings[key].blocked_context_seq != Some(agent.state_change_seq)
        {
            let response = self
                .herdr
                .herdr(
                    "agent.read",
                    json!({"target":agent.pane_id,"source":"visible","format":"text","lines":40}),
                )
                .await?;
            herdr::get(&self.herdr, agent).await?;
            if let Some(text) = response["result"]["read"]["text"].as_str() {
                let text = format!("> blocked Agent needs input (terminal context):\n{text}");
                if self.config.edit_responses {
                    self.state.update_response(key, &group, &text, false);
                } else {
                    self.state.enqueue(&group, &text);
                }
            }
            self.state
                .mappings
                .get_mut(key)
                .unwrap()
                .blocked_context_seq = Some(agent.state_change_seq);
        }
        if agent.settled()
            && (self.state.mappings[key].completion_pending
                || now().saturating_sub(self.state.mappings[key].last_export_at)
                    >= self.config.settled_reread_secs)
        {
            self.collect_finals(key, agent, &group).await?;
            if self.config.edit_responses {
                if self.state.mappings[key].response_id.is_some() {
                    self.state.update_response(
                        key,
                        &group,
                        "> idle Waiting for the completed reply.",
                        false,
                    );
                }
                if agent.agent_session.is_some() {
                    self.state.mappings.get_mut(key).unwrap().task_revision = self
                        .read_tasks(agent)
                        .await
                        .ok()
                        .flatten()
                        .map(|tasks| tasks.revision);
                }
            }
            if self.state.mappings[key].completion_pending {
                let awaiting_final = self.config.edit_responses
                    && self.state.mappings[key].response_id.is_some()
                    && agent
                        .agent_session
                        .as_ref()
                        .is_some_and(|session| output::supported(&session.agent));
                self.state.mappings.get_mut(key).unwrap().pending_prompt = awaiting_final;
                self.state.mappings.get_mut(key).unwrap().completion_pending = awaiting_final;
            }
        }
        let m = self.state.mappings.get_mut(key).unwrap();
        m.last_agent_status = agent.agent_status.clone();
        m.last_state_change_seq = agent.state_change_seq;
        m.active = true;
        m.agent_record = Some(agent.clone());
        self.save()?;
        Ok(())
    }

    fn observe_activity(&mut self, key: &str, history: &output::SessionHistory) -> Result<()> {
        if !self.config.auto_forwarding && !self.config.phone_replies_only {
            return Ok(());
        }
        let previous = self.state.mappings[key].activity.clone();
        if self
            .state
            .mappings
            .get_mut(key)
            .unwrap()
            .activity
            .observe(&history.user_inputs)
            && self.config.auto_forwarding
        {
            if let Err(error) = forwarding::set(&self.config.state_path, false) {
                self.state.mappings.get_mut(key).unwrap().activity = previous;
                // Keep outgoing delivery closed even if the control write failed.
                self.state.forwarding_enabled = false;
                return Err(error);
            }
            self.refresh_forwarding()?;
            eprintln!("Local native user input muted White Noise forwarding for all groups.");
        }
        self.save()
    }

    fn activity_unresolved(&self) -> bool {
        self.config.auto_forwarding
            && self
                .state
                .mappings
                .values()
                .any(|mapping| mapping.activity.unresolved())
    }

    async fn read_history(&mut self, agent: &Agent) -> Result<output::SessionHistory> {
        Ok(self
            .reads
            .read(&self.config, agent, self.state.forwarding_revision.clone())
            .await
            .map_err(read_failed)?
            .history
            .clone())
    }

    async fn read_tasks(&mut self, agent: &Agent) -> Result<Option<output::tasks::TaskList>> {
        Ok(self
            .reads
            .read(&self.config, agent, self.state.forwarding_revision.clone())
            .await
            .map_err(read_failed)?
            .tasks
            .clone())
    }

    /// Schedule independent transcript reads without waiting for their completion.
    pub async fn poll_sessions(&mut self, agents: Vec<Agent>) -> Result<()> {
        self.reads.background = true;
        self.reconcile(agents).await
    }

    /// Applies a batch of OpenRig events. Hook states patch the last snapshot, and anything else reads `rig ps` again.
    pub async fn apply_events(&mut self, events: Vec<openrig::Event>) -> Result<()> {
        let mut resync = false;
        let mut statuses = BTreeMap::new();
        let mut stopped = HashSet::new();
        for event in events {
            if event.seq.is_some() {
                self.state.openrig_event_seq = event.seq;
            }
            match event.action {
                openrig::Action::Ignore => {}
                openrig::Action::Resync => resync = true,
                openrig::Action::Status { seat, status, stop } => {
                    if stop {
                        stopped.insert(seat.clone());
                    }
                    statuses.insert(seat, (status, event.seq.unwrap_or_default()));
                }
            }
        }
        let mut agents = self.last_snapshot.clone();
        resync |= statuses
            .keys()
            .any(|seat| !agents.iter().any(|agent| agent.seat.as_ref() == Some(seat)));
        if resync {
            agents = self.snapshot().await?;
        } else if statuses.is_empty() {
            return Ok(());
        } else {
            for agent in &mut agents {
                if let Some((status, seq)) = agent.seat.as_ref().and_then(|seat| statuses.get(seat))
                {
                    agent.agent_status.clone_from(status);
                    agent.state_change_seq = *seq;
                }
            }
        }
        let keys: Vec<_> = agents
            .iter()
            .filter(|agent| {
                agent
                    .seat
                    .as_ref()
                    .is_some_and(|seat| stopped.contains(seat))
            })
            .map(Agent::key)
            .collect();
        let finals = |bridge: &Self, key: &String| {
            bridge
                .state
                .mappings
                .get(key)
                .map_or(0, |mapping| mapping.final_ids.len())
        };
        let before: Vec<_> = keys.iter().map(|key| finals(self, key)).collect();
        let result = self.reconcile(agents).await;
        let start = tokio::time::Instant::now();
        for (key, before) in keys.into_iter().zip(before) {
            if finals(self, &key) == before {
                self.stop_rereads.insert(
                    key,
                    StopReread {
                        finals: before,
                        started: start,
                        next: start + Duration::from_secs(1),
                        wait: Duration::from_secs(1),
                    },
                );
            }
        }
        result
    }

    pub fn stop_rereads_pending(&self) -> bool {
        !self.stop_rereads.is_empty()
    }

    /// Rereads a stopped seat's transcript after 1, 2, 4… seconds until a final appears or `settled_reread_secs` pass.
    pub async fn reread_stopped(&mut self) -> Result<()> {
        let now = tokio::time::Instant::now();
        let limit = Duration::from_secs(self.config.settled_reread_secs);
        let state = &self.state;
        self.stop_rereads.retain(|key, reread| {
            state
                .mappings
                .get(key)
                .is_some_and(|mapping| mapping.final_ids.len() == reread.finals)
                && now.duration_since(reread.started) < limit
        });
        let mut due = false;
        for (key, reread) in &mut self.stop_rereads {
            if reread.next <= now {
                due = true;
                reread.wait *= 2;
                reread.next = now + reread.wait;
                self.state.mappings.get_mut(key).unwrap().completion_pending = true;
            }
        }
        if !due {
            return Ok(());
        }
        let result = self.reconcile(self.last_snapshot.clone()).await;
        let state = &self.state;
        self.stop_rereads.retain(|key, reread| {
            state
                .mappings
                .get(key)
                .is_some_and(|mapping| mapping.final_ids.len() == reread.finals)
        });
        result
    }

    pub async fn poll_ready_sessions(&mut self) -> Result<()> {
        // Completion wakes apply existing reads without starting another polling round.
        // Preserve unknown-session terminals from the authoritative HERDR snapshot.
        self.reads.scheduling = false;
        let result = self.reconcile(self.last_snapshot.clone()).await;
        self.reads.scheduling = true;
        result
    }

    pub async fn inbound(&mut self, event: &Value) -> Result<()> {
        self.route_inbound(event, false).await
    }

    async fn route_inbound(&mut self, event: &Value, prepared: bool) -> Result<()> {
        ensure!(
            event["marmot_agent_control"] == PROTOCOL,
            "MDK event protocol mismatch"
        );
        if event["type"] == "reaction_added" {
            self.reaction(event);
            return Ok(());
        }
        if event["type"] != "inbound_message"
            || event["account_id_hex"] != self.config.account_id_hex
        {
            return Ok(());
        }
        let msg = &event["message"];
        if msg["sender"]["account_id_hex"] != self.phone || msg["sender"]["is_self"] == true {
            return Ok(());
        }
        let Some(group) = event["group_id_hex"].as_str() else {
            return Ok(());
        };
        let Some(key) = self
            .state
            .mappings
            .iter()
            .find(|(_, m)| m.group_id_hex.as_deref() == Some(group))
            .map(|(k, _)| k.clone())
        else {
            return Ok(());
        };
        let Some(id) = msg["message_id_hex"].as_str() else {
            return Ok(());
        };
        if self.state.mappings[&key]
            .seen_messages
            .iter()
            .any(|seen| seen == id)
        {
            return Ok(());
        }
        if let Some(pending) = self.state.media_pending.iter_mut().find(|pending| {
            !prepared
                && pending["group_id_hex"] == group
                && pending["message"]["message_id_hex"] == id
        }) {
            if !crate::media::complete_refs(&pending["message"]) && crate::media::complete_refs(msg)
            {
                pending["message"] = msg.clone();
                pending["_codr_retry_at"] = json!(0);
                self.save()?;
            }
            return Ok(());
        }
        self.refresh_forwarding()?;
        forwarding::wake(&self.config.state_path, group, id)?;
        self.refresh_forwarding()?;
        ensure!(
            self.herdr_available,
            "HERDR unavailable; inbound message deferred until recovery"
        );
        self.activate_forwarding().await?;
        ensure!(
            !self.state.forwarding_resume_pending,
            "Forwarding resume awaits its current history checkpoint"
        );
        ensure!(
            self.agents.contains_key(&key) || !self.state.mappings[&key].active,
            "Session identity is uncertain; phone input deferred"
        );
        if let Some(agent) = self.agents.get(&key).cloned() {
            self.baseline_forwarding(&key, &agent).await?;
        }
        let text = msg["text"].as_str().unwrap_or("");
        if text != "/status"
            && !text.trim().is_empty()
            && msg["text_truncated"] != true
            && msg["attachments_truncated"] != true
            && msg["media"].as_array().is_none_or(|media| media.is_empty())
            && let Some(agent) = self.agents.get(&key).cloned()
            && self.answer_decision(&key, &agent, group, id, text).await?
        {
            return Ok(());
        }
        // A final transcript record can lag HERDR's idle state. Resolve it before
        // opening another response, so it cannot overwrite the next prompt's message.
        if self.config.edit_responses
            && text != "/status"
            && self.state.mappings[&key].pending_prompt
            && self.state.mappings[&key].response_id.is_some()
            && let Some(agent) = self.agents.get(&key).cloned().filter(Agent::settled)
        {
            self.collect_finals(&key, &agent, group).await?;
            if self.state.mappings[&key].response_id.is_none() {
                let mapping = self.state.mappings.get_mut(&key).unwrap();
                mapping.pending_prompt = false;
                mapping.completion_pending = false;
            }
            self.save()?;
        }
        if text != "/status"
            && !text.trim().is_empty()
            && msg["text_truncated"] != true
            && msg["attachments_truncated"] != true
            && msg["media"].as_array().is_none_or(|a| a.is_empty())
            && self.agents.get(&key).is_some_and(Agent::settled)
            && !self.state.mappings[&key].pending_prompt
        {
            let agent = self.agents[&key].clone();
            match self.current_agent(&agent).await {
                Ok(current) if current.settled() => {}
                Err(error) if error.downcast_ref::<crate::wire::NotSubmitted>().is_some() => {
                    self.herdr_available = false;
                    return Err(error);
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Agent readiness changed; recover message after a fresh snapshot"
                    ));
                }
            }
        }
        let media_busy = self.state.media_pending.iter().any(|pending| {
            pending["group_id_hex"] == group
                && !(prepared && pending["message"]["message_id_hex"] == id)
        });
        let media_full = media_busy || self.state.media_pending.len() >= 16;
        let image_blocked = self.config.backend == Backend::Herdr
            && !crate::media::audio(msg)
            && self
                .agents
                .get(&key)
                .is_some_and(|agent| agent.agent_status == "blocked");
        if text != "/status"
            && msg["text_truncated"] != true
            && msg["attachments_truncated"] != true
            && msg["media"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
            && !media_full
            && self.agents.contains_key(&key)
            && !image_blocked
        {
            let mut pending = event.clone();
            if crate::media::audio(msg) && self.config.edit_responses {
                pending["_codr_transcription_notice"] =
                    json!(self.state.begin_transcription(group));
            } else {
                self.state.enqueue(
                    group,
                    if crate::media::audio(msg) {
                        "> transcribing"
                    } else {
                        "> downloading"
                    },
                );
            }
            self.state.media_pending.push(pending);
            return self.save();
        }
        // Phone-only replies need the checkpoint while the agent works, because the
        // agent holds the typed prompt and answers it in a later or the running turn.
        let phone_history = if (self.config.phone_replies_only
            || (self.config.auto_forwarding
                && !self.state.mappings[&key].pending_prompt
                && self.agents.get(&key).is_some_and(Agent::settled)))
            && text != "/status"
            && !text.trim().is_empty()
            && msg["text_truncated"] != true
            && msg["attachments_truncated"] != true
            && msg["media"].as_array().is_none_or(|media| media.is_empty())
            && !media_busy
            && self.agents.get(&key).is_some_and(|agent| {
                agent
                    .agent_session
                    .as_ref()
                    .is_some_and(|session| output::supported(&session.agent))
            }) {
            let revision = self.state.forwarding_revision.clone();
            let session = self.agents[&key].agent_session.as_ref().unwrap().clone();
            let history = self
                .reads
                .fresh(&self.config, &session)
                .await
                .map_err(read_failed)?
                .history;
            self.refresh_forwarding()?;
            ensure!(
                self.state.forwarding_enabled && self.state.forwarding_revision == revision,
                "Forwarding selection changed; phone input deferred before submission"
            );
            Some(history)
        } else {
            None
        };
        // Persist consumption before terminal input: uncertain prompts must not run twice.
        let m = self.state.mappings.get_mut(&key).unwrap();
        m.seen_messages.push(id.into());
        self.save()?;
        let response = if text == "/status" {
            Some(format!(
                "> {} {} session: {}.",
                self.state.mappings[&key].last_agent_status,
                self.backend_name(),
                if self.agents.contains_key(&key) {
                    "connected"
                } else {
                    "ended"
                }
            ))
        } else if msg["text_truncated"] == true || msg["attachments_truncated"] == true {
            Some("This offline message was truncated by MDK. No prompt was sent. Send a shorter text message.".into())
        } else if !msg["media"].as_array().is_none_or(|a| a.is_empty()) {
            Some(
                if image_blocked && !media_full {
                    "The agent is waiting on a terminal prompt. Answer it at the desk, then resend."
                } else {
                    "Media could not be queued. Another media message may still be in preparation, or the session has ended. Resend it later."
                }
                .into(),
            )
        } else if let Some(agent) = self.agents.get(&key).cloned() {
            if media_busy {
                Some(
                    "Media from this group is still being prepared. Wait for it, then resend."
                        .into(),
                )
            } else if agent.agent_status == "blocked" && self.config.backend == Backend::Herdr {
                // OpenRig can keep needs_input after a prompt is answered at the desk.
                // rig send refuses with target_needs_input while a prompt is open.
                Some("The agent is waiting on a terminal prompt. Answer it at the desk, then resend.".into())
            } else if text.trim().is_empty() {
                Some("Send a non-empty text message.".into())
            } else {
                let mapping = self.state.mappings.get_mut(&key).unwrap();
                mapping.pending_prompt = true;
                if let Some(history) = phone_history {
                    mapping.activity.start_phone(id, text, &history.user_inputs);
                    if self.config.phone_replies_only {
                        mapping.activity.hold_phone(id, now());
                    }
                }
                self.save()?;
                match self.prompt_agent(&agent, text).await {
                    Ok(_) => {
                        self.state
                            .mappings
                            .get_mut(&key)
                            .unwrap()
                            .activity
                            .confirm_phone(id);
                        self.save()?;
                        if self.config.edit_responses {
                            if self.state.mappings[&key].response_id.is_some() {
                                self.state.update_response(
                                    &key,
                                    group,
                                    "> idle No completed reply was recorded.",
                                    true,
                                );
                            }
                            self.state.begin_response(&key, group);
                        }
                        None
                    }
                    Err(error) if error.downcast_ref::<openrig::Refused>().is_some() => {
                        let reason = &error.downcast_ref::<openrig::Refused>().unwrap().0;
                        let notice = if reason == "target_needs_input" {
                            "The agent is waiting on a terminal prompt. Answer it at the desk, then resend.".into()
                        } else {
                            format!(
                                "OpenRig did not type the prompt ({reason}). Resend it when the seat accepts input."
                            )
                        };
                        let m = self.state.mappings.get_mut(&key).unwrap();
                        m.pending_prompt = false;
                        m.activity.cancel_phone(id);
                        Some(notice)
                    }
                    Err(error) => {
                        if error.downcast_ref::<crate::wire::NotSubmitted>().is_some() {
                            let m = self.state.mappings.get_mut(&key).unwrap();
                            m.pending_prompt = false;
                            m.activity.cancel_phone(id);
                            m.seen_messages.retain(|seen| seen != id);
                            self.herdr_available = false;
                            self.save()?;
                            return Err(error);
                        }
                        Some("The prompt could not be confirmed. It will not be retried. Inspect the agent before submitting again. The local clear-prompt command can release an uncertain prompt.".into())
                    }
                }
            }
        } else {
            Some("This session has ended. No prompt was sent.".into())
        };
        if let Some(text) = response {
            self.state.enqueue(group, &text);
        }
        self.save()
    }

    fn reaction(&mut self, event: &Value) {
        let (Some(emoji), Some(group), Some(id)) = (
            self.config.speak_reaction.as_deref(),
            event["group_id_hex"].as_str(),
            event["target_message_id_hex"].as_str(),
        ) else {
            return;
        };
        if event["account_id_hex"] != self.config.account_id_hex
            || event["actor"]["account_id_hex"] != self.phone
            || event["emoji"] != emoji
            || event["target"]["sender"]["account_id_hex"] != self.config.account_id_hex
            || !self
                .state
                .mappings
                .values()
                .any(|m| m.group_id_hex.as_deref() == Some(group))
            || self.state.spoken_messages.iter().any(|spoken| spoken == id)
            || self
                .speech_jobs
                .iter()
                .any(|speech| speech.message_id == id)
        {
            return;
        }
        let config = self.config.clone();
        let socket = self.marmot.clone();
        let (group, message_id) = (group.to_owned(), id.to_owned());
        let job = tokio::spawn({
            let message_id = message_id.clone();
            async move { crate::speech::speak_or_report(&config, &socket, &group, &message_id).await }
        });
        self.speech_jobs.push(SpeechJob { message_id, job });
    }

    /// Records finished speech. A failed message is spoken again on a new reaction.
    pub async fn process_speech(&mut self) -> Result<()> {
        let (finished, running): (Vec<_>, Vec<_>) = std::mem::take(&mut self.speech_jobs)
            .into_iter()
            .partition(|speech| speech.job.is_finished());
        self.speech_jobs = running;
        for speech in finished {
            match speech.job.await {
                Ok(Ok(())) => {
                    self.state.spoken_messages.push(speech.message_id);
                    let excess = self
                        .state
                        .spoken_messages
                        .len()
                        .saturating_sub(SPOKEN_LIMIT);
                    self.state.spoken_messages.drain(..excess);
                }
                result => {
                    let error = result.map_or_else(anyhow::Error::from, |r| r.unwrap_err());
                    if crate::speech::nothing_to_speak(&error) {
                        eprintln!("Speech skipped: nothing to speak.");
                    } else {
                        eprintln!("Speech failed: {error:#}");
                    }
                }
            }
            self.save()?;
        }
        Ok(())
    }

    /// Process one download/transcription at a time without blocking other sessions.
    pub async fn process_media(&mut self) -> Result<()> {
        let mut routing_result = Ok(());
        if self.media_job.as_ref().is_some_and(|job| job.is_finished()) {
            let result = self.media_job.take().unwrap().await;
            if matches!(&result, Ok(Err(error)) if error.downcast_ref::<crate::media::MediaUnavailable>().is_some())
            {
                // A live event may have hydrated a summary-only history row while
                // lookup was in flight. Never consume an incomplete recovery row.
                if !crate::media::complete_refs(&self.state.media_pending[0]["message"]) {
                    self.state.media_pending[0]["_codr_retry_at"] = json!(now() + 60);
                    self.save()?;
                    return Ok(());
                }
            } else {
                match result {
                    Ok(Ok(prepared)) => {
                        // Retain references and prepared text until inbound consumption.
                        self.state.media_pending[0]["_codr_prepared_text"] = json!(prepared.prompt);
                        if let Some(transcript) = prepared.transcript {
                            let event = self.state.media_pending[0].clone();
                            self.finish_transcription_notice(
                                &event,
                                &crate::media::transcript_notice(&transcript),
                            );
                        }
                        self.save()?;
                    }
                    _ => {
                        let event = self.state.media_pending.remove(0);
                        self.finish_transcription_notice(&event, "> transcription failed");
                        let group = event["group_id_hex"].as_str().unwrap_or("");
                        if let Some(mapping) = self
                            .state
                            .mappings
                            .values_mut()
                            .find(|m| m.group_id_hex.as_deref() == Some(group))
                        {
                            if let Some(id) = event["message"]["message_id_hex"].as_str() {
                                mapping.seen_messages.push(id.into());
                            }
                            self.state.enqueue(group, "Media processing failed. No prompt was sent. Check the local media/transcription service and send the message again.");
                        }
                        self.save()?;
                    }
                }
            }
        }
        if self.media_job.is_none()
            && let Some(index) = self.state.media_pending.iter().position(|event| {
                event["_codr_prepared_text"].is_string()
                    && event["_codr_retry_at"].as_u64().unwrap_or(0) <= now()
            })
        {
            let mut event = self.state.media_pending[index].clone();
            event["message"]["text"] = event["_codr_prepared_text"].clone();
            event["message"]["media"] = json!([]);
            let result = self.route_inbound(&event, true).await;
            if self.state.mappings.values().any(|mapping| {
                mapping.group_id_hex.as_deref() == event["group_id_hex"].as_str()
                    && mapping
                        .seen_messages
                        .iter()
                        .any(|id| Some(id.as_str()) == event["message"]["message_id_hex"].as_str())
            }) {
                self.state.media_pending.remove(index);
                self.save()?;
            } else if result.is_err() {
                // Defer this session without starving prepared input for other chats.
                let mut pending = self.state.media_pending.remove(index);
                pending["_codr_retry_at"] = json!(now() + 1);
                self.state.media_pending.push(pending);
                self.save()?;
            }
            routing_result = result;
        }
        if self.media_job.is_none()
            && let Some(index) = self.state.media_pending.iter().position(|event| {
                !event["_codr_prepared_text"].is_string()
                    && event["_codr_retry_at"].as_u64().unwrap_or(0) <= now()
            })
        {
            // A deferred message must not block preparation for other chats.
            self.state.media_pending.rotate_left(index);
            let event = &self.state.media_pending[0];
            let config = self.config.clone();
            let socket = self.marmot.clone();
            let event = event.clone();
            self.media_job = Some(tokio::spawn(async move {
                crate::media::prepare(
                    &config,
                    &socket,
                    event["group_id_hex"].as_str().unwrap_or(""),
                    &event["message"],
                )
                .await
            }));
        }
        routing_result
    }

    fn finish_transcription_notice(&mut self, event: &Value, text: &str) {
        if !crate::media::audio(&event["message"]) {
            return;
        }
        if let Some(id) = event["_codr_transcription_notice"].as_str() {
            if let Some(item) = self.state.outbox.iter_mut().find(|item| item.id == id)
                && let Some(delivery) = item.response.as_mut()
            {
                item.text = text.into();
                delivery.finished = true;
            }
        } else {
            self.state
                .enqueue(event["group_id_hex"].as_str().unwrap_or(""), text);
        }
    }

    fn announce_decision(
        &mut self,
        key: &str,
        group: &str,
        mut decision: output::decision::Decision,
    ) -> Result<()> {
        if let Some(previous) = self.state.mappings[key].decision.as_ref() {
            if previous.same_request(&decision) {
                decision = previous.clone();
            } else {
                decision.requires_token = !previous.submitted;
            }
        }
        if !decision.announced && self.state.forwarding_enabled {
            self.state.enqueue(group, &decision.text());
            decision.announced = true;
            if self.config.edit_responses {
                self.state.update_response(
                    key,
                    group,
                    "> blocked Waiting for your decision.",
                    false,
                );
            }
        }
        self.state.mappings.get_mut(key).unwrap().decision = Some(decision);
        self.save()
    }

    fn decision_notice(&mut self, key: &str, group: &str, id: &str, text: &str) -> Result<bool> {
        self.state
            .mappings
            .get_mut(key)
            .unwrap()
            .seen_messages
            .push(id.into());
        self.state.enqueue(group, text);
        self.save()?;
        Ok(true)
    }

    async fn answer_decision(
        &mut self,
        key: &str,
        agent: &Agent,
        group: &str,
        id: &str,
        text: &str,
    ) -> Result<bool> {
        let Some(session) = agent
            .agent_session
            .as_ref()
            .filter(|session| session.agent == "opencode")
        else {
            return Ok(false);
        };
        let previous = self.state.mappings[key].decision.clone();
        let numeric = text.trim().bytes().all(|byte| byte.is_ascii_digit());
        let looks_like_answer = numeric
            || text.trim().starts_with("/answer ")
            || previous
                .as_ref()
                .is_some_and(|decision| decision.looks_like_answer(text));
        if agent.agent_status != "blocked" && previous.is_none() && !looks_like_answer {
            return Ok(false);
        }
        let current = herdr::get(&self.herdr, agent).await?;
        if current.agent_status != "blocked" {
            if previous.is_some() && looks_like_answer {
                return self.decision_notice(
                    key,
                    group,
                    id,
                    "That decision is no longer waiting. No answer was sent.",
                );
            }
            return Ok(false);
        }
        let active = if self.config.opencode_db.is_some() {
            self.reads
                .fresh(&self.config, session)
                .await
                .map(|snapshot| snapshot.decision)
        } else {
            output::decision::read(&self.config, session).await
        }
        .map_err(read_failed)?;
        let Some(active) = active else {
            if looks_like_answer || previous.is_some() {
                return self.decision_notice(key, group, id, "No supported decision is available yet. Wait for its question card or use the terminal. No answer was sent.");
            }
            return Ok(false);
        };
        if previous
            .as_ref()
            .is_none_or(|previous| !previous.announced || !previous.same_request(&active))
        {
            self.announce_decision(key, group, active)?;
            return self.decision_notice(key, group, id, "The current decision is shown above. Please reply to that question; no answer was sent yet.");
        }
        let decision = previous.unwrap();
        if decision.submitted {
            return self.decision_notice(key, group, id, "An answer was already submitted for this question. Check the terminal if it is still waiting.");
        }
        let choice = match decision.choice(text) {
            Ok(choice) => choice,
            Err(error) => return self.decision_notice(key, group, id, &error.to_string()),
        };
        let visible = self.herdr.herdr("agent.read", json!({"target":agent.pane_id,"source":"visible","format":"text","lines":300,"strip_ansi":true})).await?;
        let matches = visible["result"]["read"]["truncated"] != true
            && visible["result"]["read"]["text"]
                .as_str()
                .is_some_and(|text| decision.matches_visible(text));
        if !matches || herdr::get(&self.herdr, agent).await?.agent_status != "blocked" {
            return self.decision_notice(key, group, id, "The terminal is no longer showing that decision's choice list. No answer was sent. Check the terminal or wait for its current question.");
        }
        let pending_prompt = self.state.mappings[key].pending_prompt;
        let mapping = self.state.mappings.get_mut(key).unwrap();
        mapping.decision.as_mut().unwrap().submitted = true;
        mapping.pending_prompt = true;
        mapping.seen_messages.push(id.into());
        self.save()?;
        match herdr::choose(&self.herdr, agent, choice).await {
            Ok(()) => {
                self.state.enqueue(group, &format!("Sent choice {choice}: {}.", decision.questions[0].options[choice-1].label));
            }
            Err(error) if error.downcast_ref::<crate::wire::NotSubmitted>().is_some() => {
                let mapping = self.state.mappings.get_mut(key).unwrap();
                mapping.decision.as_mut().unwrap().submitted = false;
                mapping.pending_prompt = pending_prompt;
                mapping.seen_messages.retain(|seen| seen != id);
                self.herdr_available = false;
                self.save()?;
                return Err(error);
            }
            Err(_) => self.state.enqueue(group, "The decision answer could not be confirmed. It will not be retried automatically. Inspect the terminal before using clear-prompt and sending a new answer."),
        }
        self.save()?;
        Ok(true)
    }

    pub async fn recover_inbound(&mut self) -> Result<()> {
        let groups: Vec<_> = self
            .state
            .mappings
            .iter()
            .filter_map(|(key, m)| m.group_id_hex.as_ref().map(|g| (key.clone(), g.clone())))
            .collect();
        let mut failed = false;
        for (_, group) in groups {
            if self.recover_group(&group).await.is_err() {
                failed = true;
            }
        }
        ensure!(!failed, "Some groups still need inbound recovery");
        Ok(())
    }

    async fn recover_group(&mut self, group: &str) -> Result<()> {
        // Sender timestamps are not ingestion cursors. Rescan retained history and deduplicate
        // by durable message ID so delayed relay deliveries cannot fall behind a watermark.
        let mut before: Option<Value> = None;
        let mut pages = Vec::new();
        loop {
            let mut req = json!({"type":"timeline_list","account_id_hex":self.config.account_id_hex,"group_id_hex":group,"limit":50});
            if let Some(cursor) = &before {
                req["before"] = cursor.clone();
            }
            let page = self.marmot.marmot(req).await?;
            ensure!(page["type"] == "timeline_page", "Invalid timeline response");
            let rows = page["messages"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Missing timeline messages"))?
                .clone();
            if rows.is_empty() {
                break;
            }
            let cursor = json!({"recorded_at":rows[0]["recorded_at"], "message_id_hex":rows[0]["message_id_hex"]});
            ensure!(
                before.as_ref() != Some(&cursor),
                "Timeline cursor did not advance"
            );
            before = Some(cursor);
            pages.push(rows);
            if page["has_more_before"] != true {
                break;
            }
        }
        for rows in pages.into_iter().rev() {
            for row in rows {
                if row["availability"] == "available" && row["kind"] == 9 {
                    self.inbound(&json!({"marmot_agent_control":PROTOCOL,"type":"inbound_message","account_id_hex":self.config.account_id_hex,"group_id_hex":group,"message":{"message_id_hex":row["message_id_hex"],"sender":row["sender"],"text":row["text"],"text_truncated":row["text_truncated"],"attachments_truncated":row["attachments_truncated"],"recorded_at":row["recorded_at"],"media":row["attachments"]}})).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn rename_groups(&mut self) -> Result<()> {
        let live: BTreeMap<_, _> = self
            .snapshot()
            .await?
            .into_iter()
            .map(|a| (a.key(), a))
            .collect();
        let keys: Vec<_> = self.state.mappings.keys().cloned().collect();
        for key in keys {
            let mapping = &self.state.mappings[&key];
            let Some(group) = mapping.group_id_hex.clone() else {
                continue;
            };
            let agent = live.get(&key).or(mapping.agent_record.as_ref()).cloned();
            let info = self.marmot.marmot(json!({"type":"group_info","account_id_hex":self.config.account_id_hex,"group_id_hex":group})).await?;
            ensure!(
                info["type"] == "group_info",
                "Unexpected MDK group response"
            );
            let old = info["subject"].as_str().unwrap_or("session");
            let name = if let Some(agent) = agent
                .as_ref()
                .filter(|a| a.tab_label.is_some() || live.contains_key(&key))
            {
                agent.group_name()
            } else {
                let label = old
                    .strip_prefix("[cd] ")
                    .or_else(|| old.strip_prefix("[codr] "))
                    .or_else(|| old.strip_prefix("HDR | "))
                    .unwrap_or(old);
                herdr::group_name(label.strip_suffix(" · codr").unwrap_or(label))
            };
            let description = agent
                .as_ref()
                .map(Agent::group_description)
                .unwrap_or_else(|| {
                    format!(
                        "Workspace: unknown\nAgent: unknown\nManaged by codr via {}.",
                        self.backend_name()
                    )
                });
            // MDK 0.13 exposes the subject but not the description. Remember only
            // confirmed metadata writes, so matching titles can still gain details.
            if info["subject"] == name
                && info["description"]
                    .as_str()
                    .or(mapping.group_description.as_deref())
                    == Some(description.as_str())
            {
                let m = self.state.mappings.get_mut(&key).unwrap();
                m.group_name = Some(name);
                m.group_description = Some(description);
                m.group_name_locked = false;
                m.rename_uncertain = false;
                self.save()?;
                continue;
            }
            // Profile updates have no idempotency key. Never retry an uncertain mutation.
            self.state.mappings.get_mut(&key).unwrap().rename_uncertain = true;
            self.save()?;
            let result = self.marmot.marmot(json!({"type":"group_profile_update","account_id_hex":self.config.account_id_hex,"group_id_hex":group,"name":name,"description":description})).await?;
            ensure!(
                result["type"] == "group_profile_updated"
                    && result["message_ids_hex"]
                        .as_array()
                        .is_some_and(|ids| !ids.is_empty()),
                "MDK did not confirm the group rename"
            );
            let info = self.marmot.marmot(json!({"type":"group_info","account_id_hex":self.config.account_id_hex,"group_id_hex":group})).await?;
            ensure!(
                info["type"] == "group_info"
                    && info["subject"] == name
                    && info["description"]
                        .as_str()
                        .is_none_or(|actual| actual == description),
                "Renamed group title could not be verified"
            );
            let m = self.state.mappings.get_mut(&key).unwrap();
            m.group_name = Some(name.clone());
            m.group_description = Some(description);
            m.group_name_locked = false;
            m.rename_uncertain = false;
            self.save()?;
            println!("Renamed: {name}");
        }
        Ok(())
    }

    /// Keep a live group's title and description in step with HERDR. A title the
    /// user changed is never overwritten, and an uncertain profile update is never retried.
    async fn sync_group_profile(&mut self, key: &str, agent: &Agent, group: &str) {
        let desired_name = agent.group_name();
        let desired_description = agent.group_description();
        let (last_name, last_description) = {
            let mapping = &self.state.mappings[key];
            if mapping.group_name_locked || mapping.rename_uncertain {
                return;
            }
            // Groups created before this field existed have no trusted title.
            // The manual rename-groups command seeds it and enables automatic renames.
            let Some(last_name) = mapping.group_name.clone() else {
                return;
            };
            (last_name, mapping.group_description.clone())
        };
        let name_changed = last_name != desired_name;
        let description_changed = last_description.as_deref() != Some(desired_description.as_str());
        if !name_changed && !description_changed {
            return;
        }
        let info = match self.group_info(group).await {
            Ok(info) => info,
            Err(error) => {
                if error.downcast_ref::<crate::wire::NotSubmitted>().is_none() {
                    eprintln!("Group profile check failed; keeping the current title.");
                }
                return;
            }
        };
        let subject = info["subject"].as_str().unwrap_or("");
        if subject != desired_name && subject != last_name {
            let mapping = self.state.mappings.get_mut(key).unwrap();
            mapping.group_name_locked = true;
            let _ = self.save();
            eprintln!("Group title was changed outside codr; leaving it unchanged.");
            return;
        }
        let current_description = info["description"].as_str().or(last_description.as_deref());
        if subject == desired_name && current_description == Some(desired_description.as_str()) {
            let mapping = self.state.mappings.get_mut(key).unwrap();
            mapping.group_name = Some(desired_name);
            mapping.group_description = Some(desired_description);
            let _ = self.save();
            return;
        }
        self.state.mappings.get_mut(key).unwrap().rename_uncertain = true;
        if self.save().is_err() {
            eprintln!("Group rename intent could not be saved; no update was submitted.");
            return;
        }
        let update = self
            .marmot
            .marmot(json!({
                "type": "group_profile_update",
                "account_id_hex": self.config.account_id_hex,
                "group_id_hex": group,
                "name": desired_name,
                "description": desired_description,
            }))
            .await;
        match update {
            Ok(result) => {
                let confirmed = result["type"] == "group_profile_updated"
                    && result["message_ids_hex"]
                        .as_array()
                        .is_some_and(|ids| !ids.is_empty());
                let verified = confirmed
                    && match self.group_info(group).await {
                        Ok(info) => {
                            info["subject"] == desired_name
                                && info["description"]
                                    .as_str()
                                    .is_none_or(|actual| actual == desired_description)
                        }
                        Err(_) => false,
                    };
                let mapping = self.state.mappings.get_mut(key).unwrap();
                if verified {
                    mapping.group_name = Some(desired_name.clone());
                    mapping.group_description = Some(desired_description);
                    mapping.rename_uncertain = false;
                    let _ = self.save();
                    eprintln!("Renamed group: {desired_name}");
                } else {
                    mapping.rename_uncertain = true;
                    let _ = self.save();
                    eprintln!(
                        "Group profile update was not confirmed; automatic renames stopped until rename-groups runs."
                    );
                }
            }
            Err(error) => {
                if error.downcast_ref::<crate::wire::NotSubmitted>().is_some() {
                    self.state.mappings.get_mut(key).unwrap().rename_uncertain = false;
                    if self.save().is_err() {
                        self.state.mappings.get_mut(key).unwrap().rename_uncertain = true;
                    }
                    return;
                }
                let mapping = self.state.mappings.get_mut(key).unwrap();
                mapping.rename_uncertain = true;
                let _ = self.save();
                eprintln!(
                    "Group profile update is uncertain; automatic renames stopped until rename-groups runs."
                );
            }
        }
    }

    async fn group_info(&self, group: &str) -> Result<Value> {
        let info = self
            .marmot
            .marmot(json!({"type":"group_info","account_id_hex":self.config.account_id_hex,"group_id_hex":group}))
            .await?;
        ensure!(
            info["type"] == "group_info",
            "Unexpected MDK group response"
        );
        Ok(info)
    }

    /// Returns false when the backend did not list its agents.
    async fn resync(&mut self) -> bool {
        match self.snapshot().await {
            Ok(agents) => {
                if self.reconcile(agents).await.is_err() {
                    eprintln!("Session reconciliation failed; will retry.");
                }
                true
            }
            Err(_) => {
                self.herdr_available = false;
                eprintln!(
                    "{} is unavailable; waiting for reconnection.",
                    self.backend_name()
                );
                false
            }
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        self.reads.background = true;
        let reads_ready = self.reads.notify.clone();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Value>(256);
        let marmot = self.marmot.clone();
        let account = self.config.account_id_hex.clone();
        let subscriber = tokio::spawn(async move {
            loop {
                if let Ok(mut reader) = marmot.subscribe(json!({"type":"subscribe_inbound","account_id_hex":account,"group_id_hex":null}), true).await {
                    if tx.send(json!({"type":"reconcile_inbound"})).await.is_err() { return; }
                    while let Ok(event) = read_line(&mut reader).await {
                        if tx.send(event).await.is_err() { return; }
                    }
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
        let (event_tx, mut events) = tokio::sync::mpsc::channel::<openrig::Event>(256);
        let follower = (self.config.backend == Backend::OpenRig).then(|| {
            let config = self.config.clone();
            let since = self.state.openrig_event_seq;
            tokio::spawn(async move { openrig::follow(&config, since, event_tx).await })
        });
        // Authoritative snapshots avoid relying on HERDR's non-durable, differently shaped event payloads.
        // State-change sequence and stored assistant message IDs also catch work completed between snapshots.
        // OpenRig events drive its seats, so its snapshot only resyncs.
        let mut tick = tokio::time::interval(Duration::from_secs(match self.config.backend {
            Backend::Herdr => 3,
            Backend::OpenRig => 60,
        }));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut recovery_due = true;
        let mut recovery_tick = tokio::time::interval(Duration::from_secs(60));
        let mut reread_tick = tokio::time::interval(Duration::from_secs(1));
        recovery_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut speech_tick = tokio::time::interval(Duration::from_secs(1));
        // A deferred phone message retries after a fresh snapshot within seconds, because the
        // OpenRig snapshot and the recovery ticks are 60 s apart.
        let mut retry = tokio::time::interval(Duration::from_secs(2));
        let mut retries = 0u8;
        let mut media_retried = false;
        loop {
            let mut inbound_deferred = false;
            let mut media_deferred = false;
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    subscriber.abort();
                    if let Some(follower) = &follower { follower.abort(); }
                    if let Some(job) = self.media_job.take() { job.abort(); }
                    for speech in &self.speech_jobs { speech.job.abort(); }
                    return Ok(());
                },
                _ = tick.tick() => {
                    if !self.resync().await { recovery_due = true; }
                },
                _ = retry.tick(), if retries > 0 => {
                    retries -= 1;
                    self.resync().await;
                    recovery_due = true;
                },
                Some(event) = events.recv() => {
                    let mut batch = vec![event];
                    while let Ok(event) = events.try_recv() { batch.push(event); }
                    if self.apply_events(batch).await.is_err() {
                        recovery_due = true;
                        eprintln!("OpenRig event handling failed; the next resync retries it.");
                    }
                },
                _ = reread_tick.tick(), if self.stop_rereads_pending() => {
                    if self.reread_stopped().await.is_err() {
                        eprintln!("A stopped seat's transcript reread failed; it retries.");
                    }
                },
                _ = reads_ready.notified() => {
                    if self.herdr_available && self.poll_ready_sessions().await.is_err() {
                        eprintln!("Session reads remain pending.");
                    }
                },
                Some(event) = rx.recv() => {
                    if event["type"] == "reconcile_inbound" || event["type"] == "resync_required" {
                        // Refresh current agent identities before recovering any queued phone prompts.
                        recovery_due = true;
                    } else if let Err(error) = self.inbound(&event).await {
                        recovery_due = true;
                        inbound_deferred = true;
                        eprintln!("Inbound message deferred; will reconcile history. Cause: {error:#}");
                    }
                },
                _ = recovery_tick.tick() => { recovery_due = true; }
                _ = speech_tick.tick(), if !self.speech_jobs.is_empty() => {}
            }
            if recovery_due && self.herdr_available {
                recovery_due = self.recover_inbound().await.is_err();
            }
            if let Err(error) = self.process_media().await {
                recovery_due = true;
                media_deferred = true;
                eprintln!(
                    "Prepared media input deferred; will reconcile history. Cause: {error:#}"
                );
            }
            // Deferred media fails again on every pass while the agent works, so it arms the
            // retries once while it waits. A recovery that succeeds leaves the media waiting.
            if inbound_deferred || (media_deferred && !media_retried) {
                retries = 3;
                retry.reset();
                media_retried |= media_deferred;
            }
            let media_waiting = self
                .state
                .media_pending
                .iter()
                .any(|event| event["_codr_prepared_text"].is_string());
            if !recovery_due && !media_waiting {
                retries = 0;
                media_retried = false;
            }
            if self.process_speech().await.is_err() {
                eprintln!("Speech result could not be saved; it retries.");
            }
            if self.flush().await.is_err() {
                eprintln!("MDK send unavailable; private outbox retained for retry.");
            }
        }
    }
}

/// Transcript errors can quote transcript text or CLI stderr, and deferral logs print the chain.
/// `Pending` passes through, because callers downcast it to stay silent.
fn read_failed(error: anyhow::Error) -> anyhow::Error {
    if error.is::<crate::reads::Pending>() {
        return error;
    }
    anyhow::anyhow!(
        "Session history read failed: {}",
        output::read_error_message(&error)
    )
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
