//! Read-only backend discovery. Never sends a model prompt or exposes credentials.
use crate::{BridgeConfig, output};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub label: String,
    pub provider: Option<String>,
    pub variant: Option<String>,
    #[serde(default)]
    pub free: bool,
}

impl Model {
    pub fn same_reference(&self, other: &Self) -> bool {
        self.id == other.id && self.provider == other.provider && self.variant == other.variant
    }
    pub fn matches_reference(&self, actual: &Value) -> bool {
        let mut expected = self.reference();
        // OpenCode makes an omitted variant explicit in session responses.
        if self.variant.is_none() && actual["variant"] == "default" {
            expected["variant"] = json!("default");
        }
        *actual == expected
    }
    pub fn reference(&self) -> Value {
        let mut value = json!({"id":self.id,"providerID":self.provider});
        if let Some(variant) = &self.variant {
            value["variant"] = json!(variant);
        }
        value
    }
    pub fn default_choice() -> Self {
        Self {
            id: String::new(),
            label: "Backend default".into(),
            provider: None,
            variant: None,
            free: false,
        }
    }
    pub fn display(&self) -> String {
        match &self.provider {
            Some(provider) => format!(
                "{} — {provider}/{}{}",
                self.label,
                self.id,
                self.variant
                    .as_ref()
                    .map(|v| format!("#{v}"))
                    .unwrap_or_default()
            ),
            None => self.label.clone(),
        }
    }
}

pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 180
        && !value.starts_with('-')
        && value.bytes().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'/' | b':' | b'+' | b'@')
        })
}

fn label(value: Option<&str>, fallback: &str) -> String {
    value
        .unwrap_or(fallback)
        .chars()
        .filter(|c| !c.is_control())
        .take(100)
        .collect()
}

pub fn opencode_models(value: &Value) -> Result<Vec<Model>> {
    let rows = value["data"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("OpenCode model catalogue is unavailable"))?;
    ensure!(rows.len() <= 2000, "Model catalogue exceeds its limit");
    let mut models = vec![];
    let mut seen = HashSet::new();
    for row in rows {
        if row["enabled"] != true || row["status"] == "deprecated" {
            continue;
        }
        let (Some(id), Some(provider)) = (row["id"].as_str(), row["providerID"].as_str()) else {
            continue;
        };
        if !valid_id(id) || !valid_id(provider) {
            continue;
        }
        let model = Model {
            id: id.into(),
            label: label(row["name"].as_str(), id),
            provider: Some(provider.into()),
            variant: None,
            free: row["cost"].as_array().is_some_and(|costs| {
                !costs.is_empty()
                    && costs.iter().all(|cost| {
                        ["input", "output"]
                            .iter()
                            .all(|key| cost[key].as_f64() == Some(0.0))
                            && ["read", "write"]
                                .iter()
                                .all(|key| cost["cache"][key].as_f64() == Some(0.0))
                    })
            }),
        };
        if seen.insert((provider.to_owned(), id.to_owned(), None)) {
            models.push(model.clone());
        }
        for variant in row["variants"].as_array().into_iter().flatten().take(32) {
            let Some(id) = variant["id"].as_str().filter(|v| valid_id(v)) else {
                continue;
            };
            if seen.insert((provider.to_owned(), model.id.clone(), Some(id.to_owned()))) {
                models.push(Model {
                    variant: Some(id.into()),
                    ..model.clone()
                });
            }
        }
    }
    ensure!(models.len() <= 4000, "Model catalogue exceeds its limit");
    Ok(models)
}

pub fn codex_models(value: &Value) -> Result<Vec<Model>> {
    let rows = value["models"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Codex model cache is unavailable"))?;
    ensure!(rows.len() <= 2000, "Model catalogue exceeds its limit");
    let mut models = vec![];
    let mut seen = HashSet::new();
    for row in rows {
        let Some(id) = row["slug"].as_str().filter(|id| valid_id(id)) else {
            continue;
        };
        if row["visibility"] != "list" || !seen.insert(id) {
            continue;
        }
        models.push(Model {
            id: id.into(),
            label: format!("{} (cached)", label(row["display_name"].as_str(), id)),
            provider: None,
            variant: None,
            free: false,
        });
    }
    Ok(models)
}

fn executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

pub fn find_executable(bin: &Path) -> Option<PathBuf> {
    if bin.components().count() > 1 {
        return executable(bin).then(|| bin.to_owned());
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(bin))
        .find(|path| executable(path))
}

pub fn installed(config: &BridgeConfig) -> Vec<String> {
    [
        ("opencode", config.opencode_bin.as_path()),
        ("codex", Path::new("codex")),
        ("claude", Path::new("claude")),
    ]
    .into_iter()
    .filter(|(_, bin)| find_executable(bin).is_some())
    .map(|(kind, _)| kind.into())
    .collect()
}

pub fn location_query(directory: &Path) -> String {
    let mut encoded = String::new();
    for byte in directory.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(*byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("location[directory]={encoded}")
}

pub async fn api(
    config: &BridgeConfig,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value> {
    let text = body.map(Value::to_string);
    let mut args = vec!["api", method, path];
    if let Some(text) = &text {
        args.extend(["--data", text]);
    }
    let output = output::run_command(&config.opencode_bin, &args, 8 * 1024 * 1024)
        .await
        .map_err(|_| anyhow::anyhow!("OpenCode API request was not confirmed"))?;
    if output.metadata()?.len() == 0 {
        return Ok(Value::Null);
    }
    serde_json::from_reader(output)
        .map_err(|_| anyhow::anyhow!("OpenCode returned an invalid response"))
}

pub async fn models(
    config: &BridgeConfig,
    kind: &str,
    directory: &Path,
    include_default: bool,
) -> Result<Vec<Model>> {
    let mut models = match kind {
        "opencode" => {
            // New folders have no configuration yet; use the nearest existing parent.
            let mut location = directory.to_owned();
            while !location.is_dir() {
                ensure!(location.pop(), "Project location is unavailable");
            }
            opencode_models(
                &api(
                    config,
                    "get",
                    &format!("/api/model?{}", location_query(&location)),
                    None,
                )
                .await?,
            )?
        }
        "codex" => {
            use std::io::Read;
            let file = std::fs::File::open(config.codex_home.join("models_cache.json"));
            match file {
                Ok(file) => {
                    let mut bytes = vec![];
                    file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
                    ensure!(
                        bytes.len() <= 8 * 1024 * 1024,
                        "Codex model cache exceeds its limit"
                    );
                    codex_models(
                        &serde_json::from_slice(&bytes)
                            .map_err(|_| anyhow::anyhow!("Codex model cache is invalid"))?,
                    )?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
                Err(_) => anyhow::bail!("Codex model cache is unreadable"),
            }
        }
        "claude" => ["sonnet", "opus", "haiku"]
            .into_iter()
            .map(|id| Model {
                id: id.into(),
                label: format!("{id} (Claude alias; account access may vary)"),
                provider: None,
                variant: None,
                free: false,
            })
            .collect(),
        _ => anyhow::bail!("Unsupported agent"),
    };
    if include_default {
        models.insert(0, Model::default_choice());
    }
    ensure!(!models.is_empty(), "No available models were reported");
    Ok(models)
}

pub fn agent_name(kind: &str) -> &str {
    match kind {
        "opencode" => "OpenCode",
        "codex" => "Codex",
        "claude" => "Claude Code",
        other => other,
    }
}
