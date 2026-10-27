//! Client-owned machine/session discovery and presentation for explicitly combined windows.

use std::collections::{BTreeMap, HashSet};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::endpoint::{ClientEndpointId, ProfileId, SavedSshEndpoint};

pub(super) const CONFIG_ENV: &str = "HERDR_FLEET_CONFIG";
static NEXT_PREFERENCE_WRITE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
const MAX_BYTES: usize = 256 * 1024;
const MAX_SESSIONS: usize = 64;
const QUERY_TIMEOUT: Duration = Duration::from_secs(8);
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Machine {
    pub id: String,
    pub label: String,
    pub target: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    pub machine: String,
    pub session: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Config {
    pub version: u32,
    pub local_machine: String,
    pub origin_session: String,
    pub selected: Selection,
    pub machines: Vec<Machine>,
}

impl Config {
    pub fn load() -> Result<Option<Self>, String> {
        let Some(path) = std::env::var_os(CONFIG_ENV) else {
            return Ok(None);
        };
        let path = PathBuf::from(path);
        let state = std::env::var_os(crate::config::CLIENT_STATE_DIR_ENV_VAR)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or("combined windows require an absolute HERDR_CLIENT_STATE_DIR")?;
        if !path.is_absolute() || path.parent() != Some(state.as_path()) {
            return Err("fleet configuration must be inside its scoped client directory".into());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .and_then(|file| file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes))
            .map_err(|error| format!("cannot read fleet configuration: {error}"))?;
        if bytes.len() > MAX_BYTES {
            return Err("fleet configuration is too large".into());
        }
        let config: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid fleet configuration: {error}"))?;
        config.validate()?;
        let active = crate::session::active_name()
            .unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.into());
        if config.origin_session != active {
            return Err("fleet origin does not match the attached local session".into());
        }
        Ok(Some(config))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != 2 || self.machines.is_empty() || self.machines.len() > 8 {
            return Err("unsupported fleet version or machine count".into());
        }
        crate::session::validate_name(&self.origin_session)?;
        crate::session::validate_name(&self.selected.session)?;
        if self.origin_session != self.selected.session {
            return Err("combined windows require one session name across machines".into());
        }
        let mut ids = HashSet::new();
        let mut local = 0;
        for machine in &self.machines {
            if machine.id.is_empty()
                || machine.id.len() > 64
                || !machine
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                || !ids.insert(machine.id.as_str())
                || machine.label.trim().is_empty()
                || machine.label.len() > 128
                || machine.label.chars().any(char::is_control)
            {
                return Err("invalid or duplicate fleet machine identity".into());
            }
            if let Some(target) = &machine.target {
                SavedSshEndpoint::new(&machine.label, target, "default")?;
            } else {
                local += 1;
                if machine.id != self.local_machine {
                    return Err("local transport belongs to the current machine only".into());
                }
            }
        }
        if local != 1 || !ids.contains(self.selected.machine.as_str()) {
            return Err("fleet needs one current machine and a configured selection".into());
        }
        Ok(())
    }

    pub fn endpoint_id(&self, machine: &str, session: &str) -> ClientEndpointId {
        if machine == self.local_machine && session == self.origin_session {
            ClientEndpointId::Local
        } else {
            let mut hash = Sha256::new();
            for part in [machine, session] {
                hash.update((part.len() as u64).to_le_bytes());
                hash.update(part.as_bytes());
            }
            let id = format!("{:x}", hash.finalize());
            ClientEndpointId::Session(ProfileId::parse(&id[..32]).expect("hex digest"))
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct DiscoveredSession {
    pub name: String,
    pub running: bool,
}

#[derive(Deserialize)]
struct SessionList {
    sessions: Vec<DiscoveredSession>,
}

#[derive(Debug)]
pub(super) struct Discovery {
    pub machine: String,
    pub epoch: u64,
    pub result: Result<Vec<DiscoveredSession>, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Transport {
    Local(PathBuf),
    Ssh(SavedSshEndpoint),
}

#[derive(Clone, Debug)]
pub(super) struct Session {
    pub endpoint: ClientEndpointId,
    pub machine: String,
    pub name: String,
    pub running: bool,
    pub transport: Transport,
}

#[derive(Clone, Debug)]
pub(super) struct MachineState {
    pub machine: Machine,
    pub epoch: u64,
    pub error: Option<String>,
    pub discovered: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum AgentScope {
    #[default]
    ThisSpace,
    ThisMachine,
    AllMachines,
}

impl AgentScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::ThisSpace => "this space",
            Self::ThisMachine => "this machine",
            Self::AllMachines => "all machines",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::ThisSpace => Self::ThisMachine,
            Self::ThisMachine => Self::AllMachines,
            Self::AllMachines => Self::ThisSpace,
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u32,
    #[serde(default)]
    scope: AgentScope,
    #[serde(default)]
    collapsed: HashSet<String>,
    selected: Option<Selection>,
}

#[derive(Clone, Debug)]
pub(super) struct State {
    pub config: Config,
    pub machines: Vec<MachineState>,
    pub sessions: BTreeMap<ClientEndpointId, Session>,
    pub scope: AgentScope,
    pub collapsed: HashSet<String>,
    pub selected: Selection,
    preference_path: Option<PathBuf>,
}

impl State {
    pub fn new(config: Config) -> Self {
        Self::from_path(config, preferences_path())
    }

    fn from_path(config: Config, path: PathBuf) -> Self {
        let loaded = (|| {
            let file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.to_string()),
            };
            let mut bytes = Vec::new();
            file.take(MAX_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            if bytes.len() > MAX_BYTES {
                return Err("preferences exceed size limit".into());
            }
            let preferences: Preferences =
                serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            if preferences.version != 1 {
                return Err("unsupported preferences version".into());
            }
            Ok(Some(preferences))
        })();
        match loaded {
            Ok(preferences) => {
                let mut state = Self::from_preferences(config, preferences);
                state.preference_path = Some(path);
                state
            }
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "preserving unreadable combined-window preferences; writes disabled");
                Self::from_preferences(config, None)
            }
        }
    }

    #[cfg(test)]
    pub(super) fn isolated(config: Config) -> Self {
        Self::from_preferences(config, None)
    }

    fn from_preferences(config: Config, preferences: Option<Preferences>) -> Self {
        let mut state = Self {
            machines: config
                .machines
                .iter()
                .cloned()
                .map(|machine| MachineState {
                    machine,
                    epoch: 0,
                    error: None,
                    discovered: false,
                })
                .collect(),
            sessions: BTreeMap::new(),
            scope: preferences
                .as_ref()
                .map_or(AgentScope::default(), |p| p.scope),
            collapsed: preferences
                .as_ref()
                .map_or_else(HashSet::new, |p| p.collapsed.clone()),
            preference_path: None,
            selected: config.selected.clone(),
            config,
        };
        state.collapsed.retain(|id| {
            state
                .config
                .machines
                .iter()
                .any(|machine| &machine.id == id)
        });
        let local = state
            .config
            .machines
            .iter()
            .find(|m| m.target.is_none())
            .cloned()
            .expect("validated local machine");
        state.insert_session(&local, state.config.origin_session.clone(), true);
        state
    }

    fn insert_session(&mut self, machine: &Machine, name: String, running: bool) {
        if name != self.config.selected.session {
            return;
        }
        let endpoint = self.config.endpoint_id(&machine.id, &name);
        let transport = match &machine.target {
            Some(target) => Transport::Ssh(SavedSshEndpoint {
                id: match &endpoint {
                    ClientEndpointId::Session(id) => id.clone(),
                    _ => unreachable!(),
                },
                label: format!("{}/{}", machine.label, name),
                target: target.clone(),
                session: name.clone(),
                enabled: running,
            }),
            None => Transport::Local(crate::session::client_socket_path_for(
                (name != crate::session::DEFAULT_SESSION_NAME).then_some(name.as_str()),
            )),
        };
        let running = running || endpoint.is_local();
        self.sessions.insert(
            endpoint.clone(),
            Session {
                endpoint,
                machine: machine.id.clone(),
                name,
                running,
                transport,
            },
        );
    }

    pub fn apply(&mut self, discovery: Discovery) -> bool {
        let Some(index) = self
            .machines
            .iter()
            .position(|m| m.machine.id == discovery.machine)
        else {
            return false;
        };
        if discovery.epoch <= self.machines[index].epoch {
            return false;
        }
        self.machines[index].epoch = discovery.epoch;
        self.machines[index].discovered = true;
        match discovery.result {
            Err(error) => self.machines[index].error = Some(error),
            Ok(sessions) => {
                self.machines[index].error = None;
                let machine = self.machines[index].machine.clone();
                let selected = self
                    .config
                    .endpoint_id(&self.selected.machine, &self.selected.session);
                let names = sessions
                    .iter()
                    .filter(|session| session.name == self.config.selected.session)
                    .map(|session| session.name.as_str())
                    .collect::<HashSet<_>>();
                self.sessions.retain(|endpoint, session| {
                    session.name == self.config.selected.session
                        && (session.machine != machine.id
                            || endpoint.is_local()
                            || endpoint == &selected
                            || names.contains(session.name.as_str()))
                });
                for session in self
                    .sessions
                    .values_mut()
                    .filter(|s| s.machine == machine.id)
                {
                    if session.endpoint != ClientEndpointId::Local {
                        session.running = false;
                    }
                }
                for session in sessions {
                    self.insert_session(&machine, session.name, session.running);
                }
            }
        }
        true
    }

    pub fn machine_for(&self, endpoint: &ClientEndpointId) -> Option<&str> {
        self.sessions
            .get(endpoint)
            .map(|session| session.machine.as_str())
    }

    pub fn label(&self, endpoint: &ClientEndpointId) -> String {
        self.sessions.get(endpoint).map_or_else(
            || "unavailable".into(),
            |session| {
                let machine = self
                    .machines
                    .iter()
                    .find(|m| m.machine.id == session.machine)
                    .expect("session machine");
                format!("{}/{}", machine.machine.label, session.name)
            },
        )
    }

    pub fn select(&mut self, endpoint: &ClientEndpointId) {
        if let Some(session) = self.sessions.get(endpoint) {
            self.selected = Selection {
                machine: session.machine.clone(),
                session: session.name.clone(),
            };
            self.persist();
        }
    }

    pub fn persist(&self) {
        let Some(path) = self.preference_path.as_ref() else {
            return;
        };
        let prefs = Preferences {
            version: 1,
            scope: self.scope,
            collapsed: self.collapsed.clone(),
            selected: Some(self.selected.clone()),
        };
        if let Ok(bytes) = serde_json::to_vec(&prefs) {
            let temp = path.with_extension(format!(
                "tmp-{}-{}",
                std::process::id(),
                NEXT_PREFERENCE_WRITE.fetch_add(1, Ordering::Relaxed)
            ));
            let mut created = false;
            let result = (|| {
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&temp)?;
                created = true;
                file.write_all(&bytes)?;
                std::fs::rename(&temp, path)
            })();
            if let Err(error) = result {
                if created {
                    let _ = std::fs::remove_file(&temp);
                }
                tracing::warn!(%error, "cannot save combined-window preferences");
            }
        }
    }
}

fn preferences_path() -> PathBuf {
    crate::config::client_state_dir().join("fleet-preferences.json")
}

pub(super) fn start(
    config: &Config,
    tx: tokio::sync::mpsc::Sender<super::events::ClientLoopEvent>,
    quit: Arc<AtomicBool>,
) {
    for machine in config.machines.clone() {
        let tx = tx.clone();
        let quit = quit.clone();
        tokio::spawn(async move {
            let mut epoch = 0;
            while !quit.load(Ordering::Acquire) {
                epoch += 1;
                let result = discover(&machine).await;
                if quit.load(Ordering::Acquire) {
                    break;
                }
                if tx
                    .send(super::events::ClientLoopEvent::FleetDiscovery(Discovery {
                        machine: machine.id.clone(),
                        epoch,
                        result,
                    }))
                    .await
                    .is_err()
                {
                    break;
                }
                tokio::time::sleep(REFRESH_INTERVAL).await;
            }
        });
    }
}

async fn discover(machine: &Machine) -> Result<Vec<DiscoveredSession>, String> {
    let mut command = if let Some(target) = &machine.target {
        let mut command = tokio::process::Command::new("ssh");
        command.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "ConnectionAttempts=1",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
            "--",
            target,
            "herdr session list --json",
        ]);
        command
    } else {
        let mut command =
            tokio::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        command.args(["session", "list", "--json"]);
        command
    };
    command
        .env_remove(CONFIG_ENV)
        .env_remove(crate::config::CLIENT_STATE_DIR_ENV_VAR)
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_CLIENT_SOCKET_PATH")
        .env_remove("HERDR_SESSION")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let outcome = tokio::time::timeout(QUERY_TIMEOUT, async {
        let (out, err) = tokio::join!(read_limited(stdout), read_limited(stderr));
        let status = child.wait().await?;
        Ok::<_, io::Error>((status, out?, err?))
    })
    .await;
    let (status, out, err) = match outcome {
        Ok(result) => result.map_err(|error| error.to_string())?,
        Err(_) => {
            let _ = child.kill().await;
            return Err("session discovery timed out".into());
        }
    };
    if out.len() > MAX_BYTES || err.len() > MAX_BYTES {
        return Err("session discovery response is too large".into());
    }
    if !status.success() {
        return Err(String::from_utf8_lossy(&err)
            .chars()
            .take(256)
            .filter(|c| !c.is_control())
            .collect());
    }
    parse_sessions(&out)
}

async fn read_limited(stream: impl tokio::io::AsyncRead + Unpin) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    stream
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    Ok(bytes)
}

fn parse_sessions(bytes: &[u8]) -> Result<Vec<DiscoveredSession>, String> {
    let list: SessionList = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid session response: {error}"))?;
    if list.sessions.len() > MAX_SESSIONS {
        return Err("too many discovered sessions".into());
    }
    let mut names = HashSet::new();
    for session in &list.sessions {
        crate::session::validate_name(&session.name)?;
        if !names.insert(session.name.as_str()) {
            return Err("duplicate discovered session".into());
        }
    }
    Ok(list.sessions)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn config() -> Config {
        Config {
            version: 2,
            local_machine: "khion".into(),
            origin_session: "skadi".into(),
            selected: Selection {
                machine: "khion".into(),
                session: "skadi".into(),
            },
            machines: vec![
                Machine {
                    id: "khion".into(),
                    label: "khion".into(),
                    target: None,
                },
                Machine {
                    id: "lumi".into(),
                    label: "lumi".into(),
                    target: Some("feltfomo@lumi".into()),
                },
            ],
        }
    }

    #[test]
    fn fleet_identity_is_machine_and_session_qualified_with_one_origin() {
        let config = config();
        assert!(config.validate().is_ok());
        assert_eq!(
            config.endpoint_id("khion", "skadi"),
            ClientEndpointId::Local
        );
        let ids = [
            config.endpoint_id("khion", "default"),
            config.endpoint_id("khion", "odyssey"),
            config.endpoint_id("lumi", "skadi"),
            config.endpoint_id("lumi", "default"),
        ];
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 4);
        assert!(ids.iter().all(|id| !id.is_local()));
    }

    #[test]
    fn fleet_config_rejects_ambiguous_or_unsafe_machine_identity() {
        let mut config = config();
        config.machines.push(config.machines[0].clone());
        assert!(config.validate().is_err());
        config.machines.pop();
        config.machines[1].target = None;
        assert!(config.validate().is_err());
        config.machines[1].target = Some("feltfomo@lumi".into());
        config.machines[1].label = "lumi\nother".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn fleet_discovery_rejects_stale_results_and_retains_sessions_on_failure() {
        let mut state = State::isolated(config());
        assert!(state.apply(Discovery {
            machine: "lumi".into(),
            epoch: 2,
            result: Ok(vec![DiscoveredSession {
                name: "skadi".into(),
                running: true
            }])
        }));
        let endpoint = state.config.endpoint_id("lumi", "skadi");
        assert!(state.sessions[&endpoint].running);
        assert!(!state.apply(Discovery {
            machine: "lumi".into(),
            epoch: 1,
            result: Ok(Vec::new())
        }));
        assert!(state.apply(Discovery {
            machine: "lumi".into(),
            epoch: 3,
            result: Err("offline".into())
        }));
        assert!(state.sessions[&endpoint].running);
        assert!(state.machines[1].error.is_some());
        assert!(state.apply(Discovery {
            machine: "lumi".into(),
            epoch: 4,
            result: Ok(Vec::new())
        }));
        assert!(!state.sessions.contains_key(&endpoint));
        assert!(state.sessions[&ClientEndpointId::Local].running);
    }

    #[test]
    fn fleet_discovery_registers_only_its_logical_session_and_rejects_old_config() {
        let mut old = config();
        old.version = 1;
        assert!(old.validate().is_err());
        old.version = 2;
        old.origin_session = "default".into();
        assert!(old.validate().is_err());
        let mut state = State::isolated(config());
        for epoch in 1..100 {
            state.apply(Discovery {
                machine: "lumi".into(),
                epoch,
                result: Ok(vec![DiscoveredSession {
                    name: format!("other-{epoch}"),
                    running: true,
                }]),
            });
            assert_eq!(state.sessions.len(), 1);
        }
        state.apply(Discovery {
            machine: "lumi".into(),
            epoch: 100,
            result: Ok(["default", "skadi", "odyssey"]
                .into_iter()
                .map(|name| DiscoveredSession {
                    name: name.into(),
                    running: true,
                })
                .collect()),
        });
        assert_eq!(state.sessions.len(), 2);
        assert!(state
            .sessions
            .values()
            .all(|session| session.name == "skadi"));
        state.selected = Selection {
            machine: "lumi".into(),
            session: "skadi".into(),
        };
        state.apply(Discovery {
            machine: "lumi".into(),
            epoch: 101,
            result: Ok(vec![DiscoveredSession {
                name: "odyssey".into(),
                running: true,
            }]),
        });
        assert_eq!(state.sessions.len(), 2);
        assert!(!state.sessions[&state.config.endpoint_id("lumi", "skadi")].running);
        assert!(!state.apply(Discovery {
            machine: "lumi".into(),
            epoch: 100,
            result: Ok(vec![DiscoveredSession {
                name: "skadi".into(),
                running: true
            }])
        }));
        assert!(!state.sessions[&state.config.endpoint_id("lumi", "skadi")].running);
    }

    #[test]
    fn fleet_preserves_malformed_or_future_preferences() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".runtime/fleet-malformed-preferences");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("state.json");
        for bytes in [
            b"broken".as_slice(),
            br#"{"version":999,"scope":"this_space","collapsed":[],"selected":null}"#,
        ] {
            std::fs::write(&path, bytes).unwrap();
            let state = State::from_path(config(), path.clone());
            assert!(state.preference_path.is_none());
            state.persist();
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn fleet_session_response_rejects_duplicate_and_invalid_names() {
        assert!(parse_sessions(
            br#"{"sessions":[{"name":"skadi","running":true},{"name":"skadi","running":false}]}"#
        )
        .is_err());
        assert!(parse_sessions(br#"{"sessions":[{"name":"../skadi","running":true}]}"#).is_err());
        assert!(parse_sessions(
            br#"{"sessions":[{"name":"default","running":true,"socket_path":"/untrusted"}]}"#
        )
        .is_ok());
    }
}
