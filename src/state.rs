use anyhow::{Result, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Mapping {
    pub group_id_hex: Option<String>,
    #[serde(default)]
    pub group_description: Option<String>,
    /// Last title this bridge applied or adopted for the group.
    #[serde(default)]
    pub group_name: Option<String>,
    /// The group title was changed outside codr; never overwrite it automatically.
    #[serde(default)]
    pub group_name_locked: bool,
    /// A profile update may have reached the group; do not retry automatically.
    #[serde(default)]
    pub rename_uncertain: bool,
    pub creating: bool,
    pub last_agent_status: String,
    pub last_state_change_seq: u64,
    pub final_ids: Vec<String>,
    pub output_initialised: bool,
    pub active: bool,
    pub pending_prompt: bool,
    pub seen_messages: Vec<String>,
    pub cursor: Option<serde_json::Value>,
    pub joined_at: u64,
    #[serde(default)]
    pub agent_record: Option<crate::herdr::Agent>,
    #[serde(default)]
    pub last_export_at: u64,
    #[serde(default)]
    pub blocked_context_seq: Option<u64>,
    #[serde(default)]
    pub completion_pending: bool,
    #[serde(default)]
    pub response_id: Option<String>,
    #[serde(default)]
    pub task_revision: Option<String>,
    #[serde(default)]
    pub forwarding_baseline_pending: bool,
    #[serde(default)]
    pub muted_turns: Vec<String>,
    #[serde(default)]
    pub decision: Option<crate::output::decision::Decision>,
    #[serde(default)]
    pub decision_checked_at: u64,
    #[serde(default)]
    pub decision_refresh_pending: bool,
    #[serde(default)]
    pub activity: crate::activity::Activity,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseDelivery {
    pub target: Option<String>,
    pub initial_text: Option<String>,
    pub sent_text: Option<String>,
    pub last_attempt_at: u64,
    pub finished: bool,
    /// Keep completed notices until delivery succeeds, unlike best-effort progress.
    #[serde(default)]
    pub durable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outgoing {
    pub id: String,
    pub group: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<ResponseDelivery>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeState {
    pub version: u32,
    pub account: Option<String>,
    pub phone: Option<String>,
    pub mappings: BTreeMap<String, Mapping>,
    pub outbox: Vec<Outgoing>,
    #[serde(default = "forwarding_default")]
    pub forwarding_enabled: bool,
    #[serde(default)]
    pub forwarding_revision: Option<String>,
    #[serde(default)]
    pub forwarding_resume_pending: bool,
    /// Authorised inbound media awaiting download/transcription. No terminal input yet.
    #[serde(default)]
    pub media_pending: Vec<serde_json::Value>,
    /// Sequence number of the last OpenRig event read, sent back as `Last-Event-ID`.
    #[serde(default)]
    pub openrig_event_seq: Option<u64>,
}

fn forwarding_default() -> bool {
    true
}

impl Default for BridgeState {
    fn default() -> Self {
        Self {
            version: 1,
            account: None,
            phone: None,
            mappings: BTreeMap::new(),
            outbox: vec![],
            forwarding_enabled: true,
            forwarding_revision: None,
            forwarding_resume_pending: false,
            openrig_event_seq: None,
            media_pending: vec![],
        }
    }
}

impl BridgeState {
    pub fn with_mapping(key: &str, group: String) -> Self {
        let mut s = Self::default();
        s.mappings.insert(
            key.into(),
            Mapping {
                group_id_hex: Some(group),
                ..Mapping::default()
            },
        );
        s
    }

    pub fn load(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let s: Self = serde_json::from_slice(&bytes)?;
                ensure!(s.version == 1, "Unsupported state version");
                Ok(s)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        save_json(path, self)
    }

    pub fn enqueue(&mut self, group: &str, text: &str) {
        if !self.forwarding_enabled {
            return;
        }
        // Keep each encrypted message below MDK's message-size ceiling.
        let mut rest = text;
        while !rest.is_empty() {
            let mut end = rest.len().min(30000);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            self.outbox.push(Outgoing {
                id: uuid::Uuid::new_v4().to_string(),
                group: group.into(),
                text: rest[..end].into(),
                response: None,
            });
            rest = &rest[end..];
        }
    }

    pub fn begin_response(&mut self, key: &str, group: &str) {
        if !self.forwarding_enabled {
            return;
        }
        if self.mappings[key]
            .response_id
            .as_ref()
            .is_some_and(|id| self.outbox.iter().any(|item| &item.id == id))
        {
            return;
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.outbox.push(Outgoing {
            id: id.clone(),
            group: group.into(),
            text: "> working".into(),
            response: Some(ResponseDelivery::default()),
        });
        self.mappings.get_mut(key).unwrap().response_id = Some(id);
    }

    pub fn begin_transcription(&mut self, group: &str) -> Option<String> {
        if !self.forwarding_enabled {
            return None;
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.outbox.push(Outgoing {
            id: id.clone(),
            group: group.into(),
            text: "> transcribing".into(),
            response: Some(ResponseDelivery {
                durable: true,
                ..ResponseDelivery::default()
            }),
        });
        Some(id)
    }

    pub fn update_response(&mut self, key: &str, group: &str, text: &str, finished: bool) {
        if !self.forwarding_enabled {
            return;
        }
        self.begin_response(key, group);
        let id = self.mappings[key].response_id.as_ref().unwrap();
        let item = self.outbox.iter_mut().find(|item| &item.id == id).unwrap();
        let mut end = text.len().min(30000);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        item.text = text[..end].into();
        item.response.as_mut().unwrap().finished = finished;
        if finished {
            self.mappings.get_mut(key).unwrap().response_id = None;
            // Preserve the full text of long terminal notices.
            self.enqueue(group, &text[end..]);
        }
    }

    pub fn complete_progress(&mut self, key: &str) {
        let Some(id) = self.mappings.get_mut(key).unwrap().response_id.take() else {
            return;
        };
        let Some(item) = self.outbox.iter_mut().find(|item| item.id == id) else {
            return;
        };
        let delivery = item.response.as_mut().unwrap();
        if delivery.target.is_none() {
            // Progress is best effort. Do not post a late working notice after the final.
            self.outbox.retain(|item| item.id != id);
            return;
        }
        item.text = if let Some(tasks) = item.text.strip_prefix("> working\n") {
            format!("> done\n{tasks}")
        } else {
            "> done".into()
        };
        delivery.finished = true;
    }
}

pub fn lock(path: &Path) -> Result<File> {
    let file = lock_file(path)?;
    file.try_lock_exclusive()
        .map_err(|_| anyhow::anyhow!("Bridge already runs for this state path"))?;
    Ok(file)
}

pub(crate) fn lock_file(path: &Path) -> Result<File> {
    let lock_path = path.with_extension("lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path)?;
    Ok(file)
}

pub(crate) fn save_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| anyhow::anyhow!("State path needs a parent"))?;
    if !parent.exists() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    tmp.write_all(&serde_json::to_vec(value)?)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
