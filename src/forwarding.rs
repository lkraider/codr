use anyhow::Result;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Auto,
    On,
    Off,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::On => "Always on",
            Self::Off => "Always off",
        }
    }
}

/// Short-lived control file, independent of the worker's routing-state lock.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forwarding {
    #[serde(default)]
    pub mode: Mode,
    pub enabled: bool,
    pub revision: String,
    #[serde(default)]
    pub wake_messages: Vec<String>,
}

impl Default for Forwarding {
    fn default() -> Self {
        Self {
            mode: Mode::Auto,
            enabled: true,
            revision: "initial".into(),
            wake_messages: Vec::new(),
        }
    }
}

fn path(state_path: &Path) -> PathBuf {
    state_path.with_extension("forwarding.json")
}

pub fn load(state_path: &Path) -> Result<Forwarding> {
    match fs::read(path(state_path)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Forwarding::default()),
        Err(error) => Err(error.into()),
    }
}

fn update(state_path: &Path, change: impl FnOnce(&mut Forwarding) -> bool) -> Result<Forwarding> {
    let control_path = path(state_path);
    let guard = crate::state::lock_file(&control_path)?;
    // Only the atomic control update holds this lock, never network or transcript I/O.
    guard.lock_exclusive()?;
    let mut current = load(state_path)?;
    let was_enabled = current.enabled;
    let was_mode = current.mode;
    if !change(&mut current) {
        return Ok(current);
    }
    if was_enabled != current.enabled || was_mode != current.mode {
        current.revision = uuid::Uuid::new_v4().to_string();
    }
    crate::state::save_json(&control_path, &current)?;
    Ok(current)
}

pub fn set(state_path: &Path, enabled: bool) -> Result<Forwarding> {
    update(state_path, |control| {
        if control.mode != Mode::Auto {
            return false;
        }
        let changed = control.enabled != enabled;
        control.enabled = enabled;
        changed
    })
}

pub fn toggle(state_path: &Path) -> Result<Forwarding> {
    update(state_path, |control| {
        if control.mode != Mode::Auto {
            return false;
        }
        control.enabled = !control.enabled;
        true
    })
}

pub fn wake(state_path: &Path, group: &str, message: &str) -> Result<Forwarding> {
    let key = serde_json::to_string(&(group, message))?;
    update(state_path, |control| {
        if control.wake_messages.contains(&key) {
            return false;
        }
        control.wake_messages.push(key);
        if control.mode != Mode::Off {
            control.enabled = true;
        }
        true
    })
}

pub fn set_mode(state_path: &Path, mode: Mode) -> Result<Forwarding> {
    update(state_path, |control| {
        let enabled = mode != Mode::Off;
        let changed = control.mode != mode || control.enabled != enabled;
        control.mode = mode;
        control.enabled = enabled;
        changed
    })
}
