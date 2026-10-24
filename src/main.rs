use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use herdr_white_noise_bridge::{
    BridgeConfig, BridgeState, bridge::Bridge, forwarding, state, wire,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "White Noise remote for local HERDR agent sessions")]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// Run in the foreground. HERDR starts this worker through the plugin manifest.
    Run,
    /// Print public routing state without message contents.
    Status,
    /// Validate live session history readers without changing bridge state.
    CheckReaders,
    /// Control global White Noise forwarding without stopping the worker.
    Forwarding {
        #[arg(value_enum, default_value = "status")]
        mode: ForwardingMode,
    },
    /// Resolve an uncertain creation. Inspect MDK groups before using --retry.
    ResolveCreate {
        key: String,
        #[arg(long, conflicts_with = "retry")]
        group: Option<String>,
        #[arg(long)]
        retry: bool,
    },
    /// Send a text notice to all mapped groups. Works while the worker runs.
    Notify {
        text: String,
        #[arg(long, conflicts_with = "seat")]
        group: Option<String>,
        /// OpenRig seat whose group gets the notice.
        #[arg(long)]
        seat: Option<String>,
    },
    /// Clear an uncertain prompt after inspecting the live agent. Does not resubmit it.
    ClearPrompt { key: String },
    /// Update mapped group names and workspace/agent descriptions.
    RenameGroups,
}

#[derive(Clone, Copy, ValueEnum)]
enum ForwardingMode {
    Status,
    Desk,
    Away,
    Toggle,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config_path = args.config.unwrap_or_else(|| {
        let root = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            });
        root.join("herdr-white-noise-bridge/config.toml")
    });
    let config = BridgeConfig::load(&config_path)?;
    if matches!(args.command, Some(Action::CheckReaders)) {
        let checked = check_readers(config).await?;
        println!("{}", serde_json::json!({"checked_sessions":checked}));
        return Ok(());
    }
    if let Some(Action::Forwarding { mode }) = args.command {
        let control = match mode {
            ForwardingMode::Status => forwarding::load(&config.state_path)?,
            ForwardingMode::Desk => forwarding::set(&config.state_path, false)?,
            ForwardingMode::Away => forwarding::set(&config.state_path, true)?,
            ForwardingMode::Toggle => forwarding::toggle(&config.state_path)?,
        };
        let state = BridgeState::load(&config.state_path)?;
        let resume_pending = control.enabled
            && (state.forwarding_resume_pending
                || state.forwarding_revision.as_deref().unwrap_or("initial") != control.revision);
        println!(
            "White Noise forwarding: {}",
            if resume_pending {
                "Away requested (waiting for the worker to checkpoint current history)"
            } else if control.enabled {
                "Away (on for all sessions)"
            } else {
                "At desk (muted; a new phone message wakes all sessions)"
            }
        );
        return Ok(());
    }
    if matches!(args.command, Some(Action::Status)) {
        let s = BridgeState::load(&config.state_path)?;
        let control = forwarding::load(&config.state_path)?;
        let resume_pending = resume_pending(&s, &control);
        let forwarding_enabled = control.enabled && s.forwarding_enabled && !resume_pending;
        for (key, m) in s.mappings {
            println!(
                "{}",
                serde_json::json!({"session":key,"group":m.group_id_hex,"creating_uncertain":m.creating,"active":m.active,"state":m.last_agent_status,"pending_prompt":m.pending_prompt,"forwarding_enabled":forwarding_enabled,"forwarding_resume_pending":resume_pending})
            );
        }
        return Ok(());
    }
    if let Some(Action::Notify { text, group, seat }) = &args.command {
        // A running worker owns state.json, so notify only reads it.
        let state = BridgeState::load(&config.state_path)?;
        let marmot = wire::Socket {
            path: config.wn_agent_socket.clone(),
        };
        let control = forwarding::load(&config.state_path)?;
        for group in notify_groups(&state, &control, group.as_deref(), seat.as_deref())? {
            for part in state::chunks(text) {
                let response = marmot.marmot(serde_json::json!({"type":"send_final","account_id_hex":config.account_id_hex,"group_id_hex":group,"text":part,"reply_to_message_id_hex":null,"idempotency_key":uuid::Uuid::new_v4().to_string()})).await?;
                let message_id = response["message_ids_hex"][0]
                    .as_str()
                    .filter(|_| response["type"] == "final_sent")
                    .ok_or_else(|| anyhow::anyhow!("MDK did not confirm publication"))?;
                println!(
                    "{}",
                    serde_json::json!({"group":group,"message_id":message_id})
                );
            }
        }
        return Ok(());
    }
    let _lock = state::lock(&config.state_path)?;
    let mut bridge = Bridge::open(config)?;
    match args.command.unwrap_or(Action::Run) {
        Action::Run => bridge.run().await,
        Action::ResolveCreate { key, group, retry } => {
            let m = bridge
                .state
                .mappings
                .get_mut(&key)
                .ok_or_else(|| anyhow::anyhow!("Unknown session key"))?;
            anyhow::ensure!(
                m.creating && m.group_id_hex.is_none(),
                "This mapping has no uncertain creation"
            );
            anyhow::ensure!(
                group.is_some() || retry,
                "Choose --group or --retry after inspecting MDK groups"
            );
            if let Some(group) = group {
                anyhow::ensure!(
                    !group.is_empty() && group.bytes().all(|b| b.is_ascii_hexdigit()),
                    "Invalid group ID"
                );
                m.group_id_hex = Some(group);
            }
            m.creating = false;
            bridge.save()
        }
        Action::ClearPrompt { key } => {
            let mapping = bridge
                .state
                .mappings
                .get_mut(&key)
                .ok_or_else(|| anyhow::anyhow!("Unknown session key"))?;
            mapping.pending_prompt = false;
            mapping.completion_pending = false;
            if let Some(decision) = mapping.decision.as_mut() {
                decision.submitted = false;
                decision.announced = false;
                mapping.decision_refresh_pending = true;
            }
            let group = mapping
                .group_id_hex
                .clone()
                .filter(|_| mapping.response_id.is_some());
            if let Some(group) = group {
                bridge.state.update_response(
                    &key,
                    &group,
                    "Response cancelled locally. No prompt was resubmitted.",
                    true,
                );
            }
            bridge.save()
        }
        Action::RenameGroups => bridge.rename_groups().await,
        Action::Status => unreachable!(),
        Action::CheckReaders => unreachable!(),
        Action::Forwarding { .. } => unreachable!(),
        Action::Notify { .. } => unreachable!(),
    }
}

fn resume_pending(state: &BridgeState, control: &forwarding::Forwarding) -> bool {
    control.enabled
        && (state.forwarding_resume_pending
            || state.forwarding_revision.as_deref().unwrap_or("initial") != control.revision)
}

fn notify_groups(
    state: &BridgeState,
    control: &forwarding::Forwarding,
    group: Option<&str>,
    seat: Option<&str>,
) -> Result<Vec<String>> {
    anyhow::ensure!(
        control.enabled && state.forwarding_enabled && !resume_pending(state, control),
        "Forwarding is At desk; switch to Away before sending a notice"
    );
    if let Some(seat) = seat {
        let key = serde_json::to_string(&("openrig", seat))?;
        let mapping = state
            .mappings
            .get(&key)
            .ok_or_else(|| anyhow::anyhow!("Unknown seat: {seat}"))?;
        let group = mapping
            .group_id_hex
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Seat {seat} has no group yet"))?;
        return Ok(vec![group]);
    }
    let groups: Vec<_> = state
        .mappings
        .values()
        .filter_map(|m| m.group_id_hex.clone())
        .filter(|id| group.is_none_or(|g| g == id))
        .collect();
    anyhow::ensure!(!groups.is_empty(), "No mapped groups exist yet");
    Ok(groups)
}

async fn check_readers(config: BridgeConfig) -> Result<usize> {
    use herdr_white_noise_bridge::{config::Backend, herdr, openrig, output, wire::Socket};
    let (agents, backend) = match config.backend {
        Backend::Herdr => (
            herdr::snapshot(&Socket {
                path: config.herdr_socket.clone(),
            })
            .await,
            "HERDR",
        ),
        Backend::OpenRig => (openrig::snapshot(&config).await, "OpenRig"),
    };
    let agents = agents.map_err(|_| {
        anyhow::anyhow!(
            "Session history check failed: {backend} snapshot is unavailable or invalid"
        )
    })?;
    let sessions: std::collections::BTreeMap<_, _> = agents
        .into_iter()
        .filter(|agent| {
            agent
                .agent_session
                .as_ref()
                .is_some_and(|session| output::supported(&session.agent))
        })
        .map(|agent| (agent.key(), agent.agent_session.unwrap()))
        .collect();
    let checked = sessions.len();
    let limit = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let mut readers = tokio::task::JoinSet::new();
    for session in sessions.into_values() {
        let config = config.clone();
        let limit = limit.clone();
        readers.spawn(async move {
            let _permit = limit.acquire_owned().await?;
            output::read_snapshot(&config, &session)
                .await
                .map_err(|error| {
                    anyhow::anyhow!(
                        "Session history check failed: {}",
                        output::read_error_message(&error)
                    )
                })?;
            Ok::<_, anyhow::Error>(())
        });
    }
    let mut failure = None;
    while let Some(result) = readers.join_next().await {
        if let Err(error) = result
            .map_err(|_| anyhow::anyhow!("Session history check failed: reader task failed"))
            .and_then(|result| result)
        {
            failure = Some(error);
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(checked)
}
