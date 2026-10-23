use crate::{BridgeConfig, herdr::Agent, output};
use anyhow::Result;
use std::{
    collections::{BTreeMap, HashSet},
    sync::{Arc, Mutex},
};
use tokio::{
    sync::{Notify, Semaphore},
    task::JoinHandle,
};

#[derive(Debug, thiserror::Error)]
#[error("Session read pending")]
pub(crate) struct Pending;

#[derive(Clone, PartialEq)]
struct Context {
    session: Option<String>,
    sequence: u64,
    status: String,
    revision: Option<String>,
    ended: bool,
}

struct Job {
    context: Context,
    task: JoinHandle<Result<output::SessionSnapshot>>,
}

/// One reader per session, with bounded parallel I/O and serial result application.
pub(crate) struct Reads {
    pub background: bool,
    pub scheduling: bool,
    pub notify: Arc<Notify>,
    limit: Arc<Semaphore>,
    jobs: BTreeMap<String, Job>,
    ready: BTreeMap<String, (Context, Arc<output::SessionSnapshot>)>,
    ended: HashSet<String>,
    pub database: Arc<Mutex<output::opencode::Reader>>,
}

impl Default for Reads {
    fn default() -> Self {
        Self {
            background: false,
            scheduling: true,
            notify: Arc::new(Notify::new()),
            limit: Arc::new(Semaphore::new(4)),
            jobs: BTreeMap::new(),
            ready: BTreeMap::new(),
            ended: HashSet::new(),
            database: Arc::new(Mutex::new(output::opencode::Reader::default())),
        }
    }
}

impl Drop for Reads {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.task.abort();
        }
    }
}

impl Reads {
    pub fn retain(&mut self, keys: &HashSet<String>) {
        self.ready.retain(|key, _| keys.contains(key));
        // Let in-flight native readers finish before releasing their concurrency slot.
        self.jobs
            .retain(|key, job| keys.contains(key) || !job.task.is_finished());
        self.ended.retain(|key| keys.contains(key));
        if let Ok(mut reader) = self.database.try_lock() {
            let sessions = keys
                .iter()
                .filter_map(|key| serde_json::from_str::<Vec<String>>(key).ok())
                .filter(|parts| parts.len() == 4 && parts[1] == "opencode")
                .map(|parts| parts[3].clone())
                .collect();
            reader.retain(&sessions);
        }
    }

    pub fn set_ended(&mut self, key: &str, ended: bool) {
        if ended {
            self.ended.insert(key.to_owned());
        } else {
            self.ended.remove(key);
        }
    }

    pub fn finish(&mut self, key: &str) {
        self.ready.remove(key);
    }

    pub async fn fresh(
        &self,
        config: &BridgeConfig,
        session: &crate::herdr::Session,
    ) -> Result<output::SessionSnapshot> {
        snapshot(config, session, self.database.clone()).await
    }

    pub async fn read(
        &mut self,
        config: &BridgeConfig,
        agent: &Agent,
        revision: Option<String>,
    ) -> Result<Arc<output::SessionSnapshot>> {
        let key = agent.key();
        let context = Context {
            session: agent
                .agent_session
                .as_ref()
                .map(|session| session.value.clone()),
            sequence: agent.state_change_seq,
            status: agent.agent_status.clone(),
            revision,
            ended: self.ended.contains(&key),
        };
        if let Some((old, snapshot)) = self.ready.get(&key)
            && old == &context
        {
            return Ok(snapshot.clone());
        }
        self.ready.remove(&key);
        if self
            .jobs
            .get(&key)
            .is_some_and(|job| job.task.is_finished())
        {
            let job = self.jobs.remove(&key).unwrap();
            let result = job
                .task
                .await
                .map_err(|_| anyhow::anyhow!("Session reader failed"))?;
            if job.context == context {
                let snapshot = Arc::new(result.inspect_err(|error| {
                    eprintln!(
                        "Session history read failed: {}.",
                        output::read_error_message(error)
                    );
                })?);
                self.ready
                    .insert(key.clone(), (context.clone(), snapshot.clone()));
                return Ok(snapshot);
            }
            // Finish stale readers before replacing them, including spawn_blocking reads.
        }
        let session = agent
            .agent_session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Session identity unavailable"))?
            .clone();
        if !self.background {
            let snapshot = Arc::new(self.fresh(config, &session).await?);
            self.ready.insert(key, (context, snapshot.clone()));
            return Ok(snapshot);
        }
        if !self.scheduling {
            return Err(Pending.into());
        }
        self.jobs.entry(key).or_insert_with(|| {
            let config = config.clone();
            let limit = self.limit.clone();
            let notify = self.notify.clone();
            let database = self.database.clone();
            Job {
                context,
                task: tokio::spawn(async move {
                    let result = async {
                        let _permit = limit.acquire_owned().await?;
                        snapshot(&config, &session, database).await
                    }
                    .await;
                    notify.notify_one();
                    result
                }),
            }
        });
        Err(Pending.into())
    }
}

pub(crate) async fn snapshot(
    config: &BridgeConfig,
    session: &crate::herdr::Session,
    database: Arc<Mutex<output::opencode::Reader>>,
) -> Result<output::SessionSnapshot> {
    if config.opencode_db.is_some() && session.agent == "opencode" {
        anyhow::ensure!(
            session.kind == "id",
            "Unsupported native session reference kind"
        );
        return output::read_opencode_snapshot(config, session, database).await;
    }
    output::read_snapshot(config, session).await
}
