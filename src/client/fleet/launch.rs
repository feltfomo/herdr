//! Opening another logical session in its own combined window.
//!
//! A combined window is pinned to one session by its fleet configuration, so switching or
//! creating a session replaces this client process with a fresh one scoped to the new session.
//! The window's machine list carries over; only the selection and origin change.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{Config, Selection, State, CONFIG_ENV};

const CONFIG_FILE: &str = "fleet.json";
// Kept apart from the launcher's own connection directories: fleet.json stores the local
// machine's target as null, so Herdr cannot recompute the launcher's key for it.
const CONNECTION_NAMESPACE: &str = "fleet-v2";

/// Everything needed to replace this client with a window on another session.
#[derive(Debug, Clone)]
pub(crate) struct LaunchRequest {
    pub(in crate::client) config: Config,
    /// Attach to an already running local session instead of letting startup create it.
    pub(in crate::client) attach_only: bool,
    /// SSH target that must explicitly create the session before the window opens.
    pub(in crate::client) create_on: Option<String>,
}

/// The window that opens `session` from the picker, or why it cannot open.
///
/// Prefers the machine this window has selected, then the local machine, then the first
/// configured machine that runs the session. A session that is stopped everywhere opens only
/// when the local machine knows it, because attaching must never start a remote session.
pub(in crate::client) fn open(fleet: &State, session: &str) -> Result<LaunchRequest, String> {
    let lists = |machine: &str, running_only: bool| {
        fleet
            .machines
            .iter()
            .find(|state| state.machine.id == machine)
            .is_some_and(|state| {
                state
                    .listed
                    .iter()
                    .any(|listed| listed.name == session && (listed.running || !running_only))
            })
    };
    let local = fleet.config.local_machine.as_str();
    let running = [fleet.selected.machine.as_str(), local]
        .into_iter()
        .chain(fleet.config.machines.iter().map(|m| m.id.as_str()))
        .find(|machine| lists(machine, true));
    let machine = match running {
        Some(machine) => machine,
        None if lists(local, false) => local,
        None => {
            return Err(format!(
                "Session {session} is not running on any machine; start it there first"
            ))
        }
    };
    Ok(LaunchRequest {
        config: window_config(&fleet.config, machine, session)?,
        attach_only: machine == local && lists(local, true),
        create_on: None,
    })
}

/// The window that creates `session` on `machine` and opens it there.
pub(in crate::client) fn create(
    fleet: &State,
    machine: &str,
    session: &str,
) -> Result<LaunchRequest, String> {
    validate_new_session(fleet, machine, session)?;
    let target = fleet
        .config
        .machines
        .iter()
        .find(|m| m.id == machine)
        .and_then(|m| m.target.clone());
    Ok(LaunchRequest {
        config: window_config(&fleet.config, machine, session)?,
        // A local session starts with the window; a remote one only through fleet-create.
        attach_only: false,
        create_on: target,
    })
}

/// The rules `herdr machine fleet-create` and the fleet configuration enforce, checked before
/// leaving the window so a typo does not cost the session.
pub(in crate::client) fn validate_new_session(
    fleet: &State,
    machine: &str,
    session: &str,
) -> Result<(), String> {
    if session == crate::session::DEFAULT_SESSION_NAME {
        return Err("default is reserved".into());
    }
    if crate::session::validate_name(session).is_err() {
        return Err("use 1-64 letters, digits, '.', '_' or '-'".into());
    }
    let state = fleet
        .machines
        .iter()
        .find(|state| state.machine.id == machine)
        .ok_or("unknown machine")?;
    let exists = state.listed.iter().any(|listed| listed.name == session)
        || (machine == fleet.config.local_machine && session == fleet.config.origin_session);
    if exists {
        return Err(format!("already exists on {}", state.machine.label));
    }
    Ok(())
}

fn window_config(base: &Config, machine: &str, session: &str) -> Result<Config, String> {
    let config = Config {
        version: base.version,
        local_machine: base.local_machine.clone(),
        origin_session: session.to_owned(),
        selected: Selection {
            machine: machine.to_owned(),
            session: session.to_owned(),
        },
        machines: base.machines.clone(),
    };
    config.validate()?;
    Ok(config)
}

/// `$XDG_STATE_HOME`, or `~/.local/state` when it is unset, empty or relative, as the XDG base
/// directory specification requires.
fn state_root(xdg_state_home: Option<OsString>, home: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(root) = xdg_state_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Ok(root);
    }
    home.filter(|home| home.is_absolute())
        .map(|home| home.join(".local").join("state"))
        .ok_or_else(|| "combined windows need an absolute XDG_STATE_HOME or home directory".into())
}

/// The client state directory owning one (machine, session) window, so its sidebar collapse,
/// agent scope and chrome preferences survive relaunches without leaking into other windows.
fn connection_dir(state_root: &Path, config: &Config) -> PathBuf {
    let selected = &config.selected;
    let identity = config
        .machines
        .iter()
        .find(|machine| machine.id == selected.machine)
        .and_then(|machine| machine.target.as_deref())
        .map_or_else(
            || format!("local\0{}", selected.machine),
            |target| format!("ssh\0{target}"),
        );
    let mut hash = Sha256::new();
    for part in [
        CONNECTION_NAMESPACE,
        config.local_machine.as_str(),
        identity.as_str(),
        selected.session.as_str(),
    ] {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    state_root
        .join("herdr")
        .join("connections")
        .join(CONNECTION_NAMESPACE)
        .join(format!("{:x}", hash.finalize()))
}

impl LaunchRequest {
    fn command(&self, executable: &Path, dir: &Path) -> std::process::Command {
        let mut command = std::process::Command::new(executable);
        command.args(["--session", self.config.origin_session.as_str()]);
        if self.attach_only {
            command.arg("client");
        }
        // This window's socket overrides point at the session being left.
        command
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_CLIENT_SOCKET_PATH")
            .env(crate::config::CLIENT_STATE_DIR_ENV_VAR, dir)
            .env(CONFIG_ENV, dir.join(CONFIG_FILE));
        command
    }

    /// Creates the remote session if requested, writes the window's configuration and replaces
    /// this process. Runs only after the terminal is restored so fleet-create reports to a
    /// normal terminal.
    pub(in crate::client) fn run(self) -> io::Result<()> {
        let root = state_root(
            std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()),
            crate::integration::home_dir().ok(),
        )
        .map_err(io::Error::other)?;
        let dir = connection_dir(&root, &self.config);
        let executable = crate::platform::launch_executable()?;
        if let Some(target) = &self.create_on {
            let status = std::process::Command::new(&executable)
                .args([
                    "machine",
                    "fleet-create",
                    target,
                    &self.config.origin_session,
                ])
                .env_remove(CONFIG_ENV)
                .env_remove(crate::config::CLIENT_STATE_DIR_ENV_VAR)
                // fleet-create never prompts, and the finished client's stdin reader may still be
                // blocked in read on the terminal until exec.
                .stdin(std::process::Stdio::null())
                .status()?;
            if !status.success() {
                return Err(io::Error::other(format!(
                    "session {} was not created on {target}; this window was closed",
                    self.config.origin_session
                )));
            }
        }
        write_config(&dir, &self.config).map_err(io::Error::other)?;
        Err(crate::platform::replace_current_process(
            self.command(&executable, &dir),
        ))
    }
}

/// Atomically replaces the window's fleet.json, refusing to overwrite one this binary cannot
/// read so an older Herdr never clobbers a newer launcher's configuration.
fn write_config(dir: &Path, config: &Config) -> Result<(), String> {
    let path = dir.join(CONFIG_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice::<Config>(&bytes)
                .map_err(|error| error.to_string())
                .and_then(|existing| existing.validate())
                .map_err(|error| {
                    format!(
                        "unsupported Herdr machine configuration at {}; leaving it unchanged: {error}",
                        path.display()
                    )
                })?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    }
    let bytes = serde_json::to_vec(config).map_err(|error| error.to_string())?;
    crate::client::endpoint::store_private_json(&path, &bytes, "fleet configuration")
}

#[cfg(test)]
mod tests {
    use super::super::tests::config;
    use super::super::{DiscoveredSession, Discovery};
    use super::*;

    fn fleet(khion: &[(&str, bool)], lumi: &[(&str, bool)]) -> State {
        let mut state = State::isolated(config());
        for (epoch, (machine, sessions)) in [("khion", khion), ("lumi", lumi)].iter().enumerate() {
            state.apply(Discovery {
                machine: (*machine).into(),
                epoch: epoch as u64 + 1,
                result: Ok(sessions
                    .iter()
                    .map(|(name, running)| DiscoveredSession {
                        name: (*name).into(),
                        running: *running,
                    })
                    .collect()),
            });
        }
        state
    }

    fn root() -> PathBuf {
        PathBuf::from("/state")
    }

    // Unix path literals; the rules themselves are platform-neutral.
    #[cfg(unix)]
    #[test]
    fn connection_dir_is_stable_and_distinct_per_machine_and_session() {
        let base = config();
        let dir = |machine: &str, session: &str| {
            connection_dir(
                &root(),
                &window_config(&base, machine, session).expect("valid"),
            )
        };
        assert_eq!(dir("khion", "skadi"), dir("khion", "skadi"));
        let dirs = [
            dir("khion", "skadi"),
            dir("lumi", "skadi"),
            dir("khion", "research"),
            dir("lumi", "research"),
        ];
        assert_eq!(
            dirs.iter().collect::<std::collections::HashSet<_>>().len(),
            4
        );
        for dir in &dirs {
            assert_eq!(
                dir.parent(),
                Some(Path::new("/state/herdr/connections/fleet-v2"))
            );
            assert_eq!(dir.file_name().map(|name| name.len()), Some(64));
        }
        let mut other_local = window_config(&base, "khion", "skadi").expect("valid");
        other_local.local_machine = "lumi".into();
        assert_ne!(connection_dir(&root(), &other_local), dirs[0]);
        // A label is presentation; renaming a machine must not orphan its window state.
        let mut relabeled = window_config(&base, "lumi", "skadi").expect("valid");
        relabeled.machines[1].label = "laptop".into();
        assert_eq!(connection_dir(&root(), &relabeled), dirs[1]);
    }

    // Unix path literals; the rules themselves are platform-neutral.
    #[cfg(unix)]
    #[test]
    fn state_root_uses_only_an_absolute_xdg_state_home() {
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(
            state_root(Some("/xdg".into()), home.clone()),
            Ok(PathBuf::from("/xdg"))
        );
        for relative in ["xdg", "./xdg", "~/state"] {
            assert_eq!(
                state_root(Some(relative.into()), home.clone()),
                Ok(PathBuf::from("/home/u/.local/state")),
                "{relative}"
            );
        }
        assert_eq!(
            state_root(None, home),
            Ok(PathBuf::from("/home/u/.local/state"))
        );
        assert!(state_root(Some("xdg".into()), Some("relative-home".into())).is_err());
        assert!(state_root(None, None).is_err());
    }

    #[test]
    fn written_config_validates_with_one_origin_and_keeps_newer_files() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(".runtime/fleet-launch-config");
        let _ = std::fs::remove_file(dir.join(CONFIG_FILE));
        let request = create(&fleet(&[("skadi", true)], &[]), "lumi", "research").expect("valid");
        write_config(&dir, &request.config).expect("write");
        let written: Config =
            serde_json::from_slice(&std::fs::read(dir.join(CONFIG_FILE)).expect("read"))
                .expect("parse");
        assert!(written.validate().is_ok());
        assert_eq!(written.origin_session, "research");
        assert_eq!(written.selected.session, written.origin_session);
        assert_eq!(written.selected.machine, "lumi");
        assert_eq!(written.local_machine, "khion");
        assert_eq!(written.machines.len(), 2);
        assert!(written.machines[0].target.is_none());

        let future = br#"{"version":3}"#;
        std::fs::write(dir.join(CONFIG_FILE), future).expect("write future");
        assert!(write_config(&dir, &request.config).is_err());
        assert_eq!(std::fs::read(dir.join(CONFIG_FILE)).expect("read"), future);
        std::fs::remove_file(dir.join(CONFIG_FILE)).expect("cleanup");
    }

    #[test]
    fn open_prefers_the_selected_machine_then_local_then_the_first_runner() {
        let selected = |state: &State, session: &str| {
            open(state, session).map(|request| request.config.selected.machine)
        };
        let both = fleet(&[("skadi", true), ("x", true)], &[("x", true)]);
        assert_eq!(selected(&both, "x").as_deref(), Ok("khion"));
        let mut on_lumi = both.clone();
        on_lumi.selected.machine = "lumi".into();
        assert_eq!(selected(&on_lumi, "x").as_deref(), Ok("lumi"));

        let remote_only = fleet(&[("x", false)], &[("x", true)]);
        assert_eq!(
            selected(&remote_only, "x").as_deref(),
            Ok("lumi"),
            "a running remote beats a stopped local copy"
        );
        let local_stopped = fleet(&[("x", false)], &[]);
        assert_eq!(selected(&local_stopped, "x").as_deref(), Ok("khion"));
        let remote_stopped = fleet(&[], &[("x", false)]);
        assert!(
            open(&remote_stopped, "x").is_err(),
            "never starts a remote session"
        );
        assert!(open(&both, "missing").is_err());
    }

    #[test]
    fn client_argument_only_attaches_to_a_running_local_session() {
        let state = fleet(&[("a", true), ("b", false)], &[("c", true), ("a", true)]);
        let attach_only = |request: LaunchRequest| request.attach_only;
        assert_eq!(open(&state, "a").map(attach_only), Ok(true));
        assert_eq!(open(&state, "b").map(attach_only), Ok(false));
        assert_eq!(open(&state, "c").map(attach_only), Ok(false));
        assert_eq!(create(&state, "khion", "new").map(attach_only), Ok(false));
        assert_eq!(create(&state, "lumi", "new").map(attach_only), Ok(false));

        let args = |request: &LaunchRequest| {
            request
                .command(Path::new("/bin/herdr"), Path::new("/d"))
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            args(&open(&state, "a").expect("a")),
            ["--session", "a", "client"]
        );
        assert_eq!(args(&open(&state, "c").expect("c")), ["--session", "c"]);
    }

    #[test]
    fn create_targets_only_remote_machines_through_fleet_create() {
        let state = fleet(&[("skadi", true)], &[]);
        assert_eq!(
            create(&state, "lumi", "new").map(|r| r.create_on),
            Ok(Some("feltfomo@lumi".into()))
        );
        assert_eq!(
            create(&state, "khion", "new").map(|r| r.create_on),
            Ok(None)
        );
        assert!(open(&state, "skadi").is_ok_and(|r| r.create_on.is_none()));
    }

    #[test]
    fn new_session_names_follow_fleet_rules_per_machine() {
        let state = fleet(
            &[("skadi", true), ("odyssey", false)],
            &[("research", true)],
        );
        for invalid in [
            "",
            "default",
            ".",
            "..",
            "a/b",
            "has space",
            &"x".repeat(65),
        ] {
            assert!(
                validate_new_session(&state, "khion", invalid).is_err(),
                "{invalid:?}"
            );
        }
        assert!(validate_new_session(&state, "khion", "odyssey").is_err());
        assert!(validate_new_session(&state, "khion", "skadi").is_err());
        assert!(validate_new_session(&state, "lumi", "research").is_err());
        assert!(validate_new_session(&state, "lumi", "odyssey").is_ok());
        assert!(validate_new_session(&state, "khion", "research").is_ok());
        assert!(validate_new_session(&state, "nowhere", "fresh").is_err());
    }

    // Unix path literals; the rules themselves are platform-neutral.
    #[cfg(unix)]
    #[test]
    fn command_clears_socket_overrides_and_scopes_the_window() {
        let state = fleet(&[("a", true)], &[]);
        let request = open(&state, "a").expect("a");
        let command = request.command(Path::new("/bin/herdr"), Path::new("/d"));
        let envs = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(envs["HERDR_SOCKET_PATH"], None);
        assert_eq!(envs["HERDR_CLIENT_SOCKET_PATH"], None);
        assert_eq!(envs[CONFIG_ENV].as_deref(), Some("/d/fleet.json"));
        assert_eq!(
            envs[crate::config::CLIENT_STATE_DIR_ENV_VAR].as_deref(),
            Some("/d")
        );
    }
}
