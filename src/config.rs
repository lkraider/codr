use anyhow::{Result, ensure};
use bech32::FromBase32;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Herdr,
    OpenRig,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeConfig {
    pub phone_npub: String,
    #[serde(default)]
    pub backend: Backend,
    /// Required for the herdr backend.
    #[serde(default)]
    pub herdr_socket: PathBuf,
    /// OpenRig session names, such as `dev-build@my-rig`, that get a phone group.
    #[serde(default)]
    pub openrig_seats: Vec<String>,
    #[serde(default = "default_rig")]
    pub rig_bin: PathBuf,
    /// Base URL of the OpenRig daemon, which serves the event stream at `/api/events`.
    #[serde(default = "default_openrig_url")]
    pub openrig_url: String,
    pub wn_agent_socket: PathBuf,
    pub account_id_hex: String,
    pub state_path: PathBuf,
    #[serde(default = "default_opencode")]
    pub opencode_bin: PathBuf,
    /// Explicit database/profile selection. None keeps the CLI export reader.
    #[serde(default)]
    pub opencode_db: Option<PathBuf>,
    #[serde(default = "default_codex_home")]
    pub codex_home: PathBuf,
    #[serde(default = "default_claude_home")]
    pub claude_home: PathBuf,
    /// Requires an MDK connector with edit_message support.
    #[serde(default)]
    pub edit_responses: bool,
    /// Local native user input mutes all groups; authorized phone input wakes them.
    #[serde(default = "automatic_default")]
    pub auto_forwarding: bool,
    /// Send a final reply only when the turn it answers holds a confirmed phone prompt.
    #[serde(default)]
    pub phone_replies_only: bool,
    /// Seconds between transcript rereads for a settled agent whose herdr state never changes.
    #[serde(default = "settled_reread_default")]
    pub settled_reread_secs: u64,
    /// Executable and fixed arguments. The downloaded audio path is appended.
    #[serde(default)]
    pub transcribe_command: Vec<String>,
    /// Executable and fixed arguments. The text arrives on stdin and the audio path is appended.
    #[serde(default)]
    pub speak_command: Vec<String>,
    /// Reaction from the phone that asks for a spoken copy of a bridge message.
    #[serde(default)]
    pub speak_reaction: Option<String>,
    /// Temporary audio files. The connector sends media only from beneath its `--media-allowed-root`.
    #[serde(default)]
    pub speak_dir: Option<PathBuf>,
}

fn automatic_default() -> bool {
    true
}

fn settled_reread_default() -> u64 {
    30
}

fn default_rig() -> PathBuf {
    "rig".into()
}

fn default_openrig_url() -> String {
    "http://127.0.0.1:7433".into()
}

fn default_opencode() -> PathBuf {
    "opencode".into()
}

fn native_home(variable: &str, directory: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(directory)
        })
}
fn default_codex_home() -> PathBuf {
    native_home("CODEX_HOME", ".codex")
}
fn default_claude_home() -> PathBuf {
    native_home("CLAUDE_CONFIG_DIR", ".claude")
}

pub fn phone_hex(npub: &str) -> Result<String> {
    let (prefix, data, variant) = bech32::decode(npub)?;
    let bytes = Vec::<u8>::from_base32(&data)?;
    ensure!(
        prefix == "npub" && variant == bech32::Variant::Bech32 && bytes.len() == 32,
        "Invalid phone npub"
    );
    Ok(hex::encode(bytes))
}

impl BridgeConfig {
    pub fn from_toml(text: &str) -> Result<Self> {
        let config: Self = toml::from_str(text)?;
        phone_hex(&config.phone_npub)?;
        ensure!(
            phone_hex(&config.phone_npub)? != config.account_id_hex,
            "The bridge requires its own account, separate from the phone"
        );
        ensure!(
            config.account_id_hex.len() == 64
                && config
                    .account_id_hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Invalid bridge account ID"
        );
        match config.backend {
            Backend::Herdr => ensure!(
                config.herdr_socket.is_absolute(),
                "Configuration paths must be absolute"
            ),
            Backend::OpenRig => ensure!(
                !config.openrig_seats.is_empty()
                    && config
                        .openrig_seats
                        .iter()
                        .all(|seat| !seat.trim().is_empty()),
                "The openrig backend requires at least one seat in openrig_seats"
            ),
        }
        for path in [
            &config.wn_agent_socket,
            &config.state_path,
            &config.codex_home,
            &config.claude_home,
        ] {
            ensure!(path.is_absolute(), "Configuration paths must be absolute");
        }
        if config.speak_reaction.is_some() {
            ensure!(
                !config.speak_command.is_empty()
                    && config
                        .speak_dir
                        .as_ref()
                        .is_some_and(|dir| dir.is_absolute()),
                "speak_reaction needs speak_command and an absolute speak_dir"
            );
        }
        if let Some(path) = &config.opencode_db {
            ensure!(
                path.is_absolute(),
                "OpenCode database path must be absolute"
            );
        }
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::from_toml(&std::fs::read_to_string(path)?)
    }
}
