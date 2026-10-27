//! Turning a plain `herdr` launch into a combined window when `[fleet]` lists this machine.
//!
//! The window itself is still described by a fleet.json handed over through
//! `HERDR_FLEET_CONFIG`; this only decides which (machine, session) to open and hands that to
//! the same launch core the session picker uses. The launched process inherits the scoped
//! environment, which is what keeps it from choosing a window again.

use std::io;

use super::launch::{self, LaunchRequest};
use super::{Config, CONFIG_ENV};
use crate::config::FleetConfig;

const USAGE_ERROR: &str = "--fleet and --new can only be used with the default launch command";

/// Launch options that pick a combined window explicitly.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct FleetLaunchArgs {
    /// Open the session on this configured machine instead of the local one.
    machine: Option<String>,
    /// Create the session on the selected machine first.
    new: bool,
}

impl FleetLaunchArgs {
    pub(crate) fn is_explicit(&self) -> bool {
        self.machine.is_some() || self.new
    }
}

/// Removes `--fleet <machine>` and `--new` from the leading launch options. They are only
/// recognised before the first command word, so a subcommand's own arguments are never taken.
pub(crate) fn extract_args(args: &[String]) -> Result<(Vec<String>, FleetLaunchArgs), String> {
    let mut cleaned = Vec::with_capacity(args.len());
    cleaned.extend(args.first().cloned());
    let mut parsed = FleetLaunchArgs::default();
    let mut command = false;
    let mut index = 1;
    while let Some(arg) = args.get(index) {
        let value = if arg == "--fleet" {
            index += 1;
            Some(args.get(index).ok_or("missing value for --fleet")?.as_str())
        } else {
            arg.strip_prefix("--fleet=")
        };
        if let Some(value) = value {
            if parsed.machine.is_some() {
                return Err("--fleet can only be specified once".into());
            }
            if value.is_empty() || value.starts_with('-') {
                return Err("--fleet requires a machine id from [fleet].machines".into());
            }
            parsed.machine = Some(value.to_owned());
        } else if arg == "--new" {
            if parsed.new {
                return Err("--new can only be specified once".into());
            }
            parsed.new = true;
        } else if arg == "--" || !arg.starts_with('-') {
            command = arg != "--";
            cleaned.extend_from_slice(&args[index..]);
            break;
        } else {
            cleaned.push(arg.clone());
        }
        index += 1;
    }
    if command && parsed.is_explicit() {
        return Err(USAGE_ERROR.into());
    }
    Ok((cleaned, parsed))
}

/// What a plain launch sees of its process environment.
#[derive(Debug, Clone)]
struct Environment {
    hostname: Option<String>,
    /// Already a scoped client: launched by this component, a picker or an external launcher.
    scoped: bool,
    /// A socket override pins one server, which a combined window would silently discard.
    socket_override: bool,
    session: String,
    /// `--session` was given on this command line rather than inherited from HERDR_SESSION.
    explicit_session: bool,
}

impl Environment {
    fn current() -> Self {
        let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
        Self {
            hostname: crate::platform::hostname(),
            scoped: set(CONFIG_ENV) || set(crate::config::CLIENT_STATE_DIR_ENV_VAR),
            socket_override: set(crate::api::SOCKET_PATH_ENV_VAR)
                || set(crate::server::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR),
            session: crate::session::active_name()
                .unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.into()),
            explicit_session: crate::session::explicit_session_requested(),
        }
    }
}

#[derive(Debug)]
enum Decision {
    Ordinary,
    Launch(LaunchRequest),
}

fn decide(
    settings: &FleetConfig,
    args: &FleetLaunchArgs,
    env: &Environment,
) -> Result<Decision, String> {
    // Without explicit options every reason not to combine falls back to the stock launch;
    // invalid config reaches the user through the ordinary window's config diagnostics.
    let ordinary = |reason: String| {
        if args.is_explicit() {
            Err(reason)
        } else {
            Ok(Decision::Ordinary)
        }
    };
    if env.scoped {
        return ordinary(format!(
            "--fleet/--new cannot reopen an already scoped window; unset {CONFIG_ENV} and {}",
            crate::config::CLIENT_STATE_DIR_ENV_VAR
        ));
    }
    if env.socket_override {
        return ordinary(format!(
            "--fleet/--new cannot be combined with {} or {}",
            crate::api::SOCKET_PATH_ENV_VAR,
            crate::server::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR
        ));
    }
    if !settings.is_configured() {
        return ordinary("--fleet/--new need [[fleet.machines]] in config.toml".into());
    }
    if let Err(error) = settings.validate() {
        return ordinary(format!("invalid [fleet] config: {error}"));
    }
    let Some(local) = settings.local_machine(env.hostname.as_deref()) else {
        return ordinary(format!(
            "this machine ({}) is not listed in [fleet].machines",
            env.hostname.as_deref().unwrap_or("unknown host name")
        ));
    };
    let machine = args.machine.as_deref().unwrap_or(&local.id);
    if !settings.machines.iter().any(|m| m.id == machine) {
        return Err(format!("unknown fleet machine {machine:?}"));
    }
    let session = env.session.as_str();
    if args.new {
        // Creating must never act on a session name inherited from the calling pane.
        if !env.explicit_session {
            return Err("--new needs --session <name>".into());
        }
        launch::validate_new_session_name(session)
            .map_err(|error| format!("--new needs a named --session: {error}"))?;
    }
    let base = Config::from_settings(settings, &local.id, &local.id, session)?;
    let request = if machine == local.id || args.new {
        launch::start(&base, machine, session)
    } else {
        // A remote session opens only if it already runs; the window never starts one.
        launch::attach(&base, machine, session, false)
    }?;
    Ok(Decision::Launch(request))
}

/// Replaces this process with the combined window the launch asks for. Returns `Ok` when the
/// launch should continue as an ordinary window.
pub(crate) fn open_configured_window(
    settings: &FleetConfig,
    args: &FleetLaunchArgs,
) -> io::Result<()> {
    match decide(settings, args, &Environment::current()).map_err(io::Error::other)? {
        Decision::Ordinary => Ok(()),
        Decision::Launch(request) => request.run(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{DiscoveredSession, Discovery, State};
    use super::*;
    use std::path::Path;

    fn settings() -> FleetConfig {
        toml::from_str::<crate::config::Config>(
            "[[fleet.machines]]\nid = \"khion\"\ntarget = \"feltfomo@khion\"\n\
             [[fleet.machines]]\nid = \"lumi\"\ntarget = \"feltfomo@lumi\"\n",
        )
        .expect("config")
        .fleet
    }

    fn env(session: &str) -> Environment {
        Environment {
            hostname: Some("khion.tailnet.ts.net".into()),
            scoped: false,
            socket_override: false,
            session: session.into(),
            explicit_session: true,
        }
    }

    fn args(machine: Option<&str>, new: bool) -> FleetLaunchArgs {
        FleetLaunchArgs {
            machine: machine.map(str::to_owned),
            new,
        }
    }

    fn launched(decision: Result<Decision, String>) -> LaunchRequest {
        match decision {
            Ok(Decision::Launch(request)) => request,
            other => panic!("expected a launch, got {other:?}"),
        }
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn plain_launch_opens_the_local_machine_with_its_target_mapped_to_local_ipc() {
        let request = launched(decide(&settings(), &args(None, false), &env("skadi")));
        assert_eq!(request.config.local_machine, "khion");
        assert_eq!(request.config.selected.machine, "khion");
        assert_eq!(request.config.origin_session, "skadi");
        assert!(request.config.machines[0].target.is_none());
        assert_eq!(
            request.config.machines[1].target.as_deref(),
            Some("feltfomo@lumi")
        );
        assert!(!request.attach_only, "a local launch starts or attaches");
        assert!(request.create_on.is_none());
        let default = launched(decide(&settings(), &args(None, false), &env("default")));
        assert_eq!(default.config.origin_session, "default");
    }

    #[test]
    fn launch_decisions_cover_local_and_remote_attach_and_new() {
        let cases = [
            (Some("khion"), false, None),
            (Some("khion"), true, None),
            (Some("lumi"), false, None),
            (Some("lumi"), true, Some("feltfomo@lumi")),
        ];
        for (machine, new, create_on) in cases {
            let request = launched(decide(&settings(), &args(machine, new), &env("research")));
            assert_eq!(Some(request.config.selected.machine.as_str()), machine);
            assert_eq!(request.create_on.as_deref(), create_on, "{machine:?} {new}");
            assert!(!request.attach_only, "{machine:?} {new}");
        }
    }

    #[test]
    fn new_requires_a_creatable_session_name_and_known_machine() {
        // Without --session the launch targets default, which fleet-create refuses to make.
        assert!(decide(&settings(), &args(Some("lumi"), true), &env("default")).is_err());
        assert!(decide(&settings(), &args(None, true), &env("default")).is_err());
        let mut inherited = env("research");
        inherited.explicit_session = false;
        assert!(decide(&settings(), &args(Some("lumi"), true), &inherited).is_err());
        assert!(decide(&settings(), &args(Some("lumi"), false), &inherited).is_ok());
        assert!(decide(&settings(), &args(Some("nowhere"), false), &env("skadi")).is_err());
    }

    #[test]
    fn scoped_processes_never_choose_a_window_again() {
        let mut scoped = env("skadi");
        scoped.scoped = true;
        assert!(matches!(
            decide(&settings(), &args(None, false), &scoped),
            Ok(Decision::Ordinary)
        ));
        assert!(decide(&settings(), &args(Some("lumi"), false), &scoped).is_err());
        let mut pinned = env("skadi");
        pinned.socket_override = true;
        assert!(matches!(
            decide(&settings(), &args(None, false), &pinned),
            Ok(Decision::Ordinary)
        ));
        assert!(decide(&settings(), &args(None, true), &pinned).is_err());
    }

    #[test]
    fn unconfigured_invalid_or_foreign_hosts_keep_stock_launches() {
        let mut duplicate = settings();
        duplicate.machines.push(duplicate.machines[0].clone());
        let mut foreign = env("skadi");
        foreign.hostname = Some("other".into());
        let mut unnamed = env("skadi");
        unnamed.hostname = None;
        for (fleet, env) in [
            (FleetConfig::default(), env("skadi")),
            (duplicate, env("skadi")),
            (settings(), foreign),
            (settings(), unnamed),
        ] {
            assert!(matches!(
                decide(&fleet, &args(None, false), &env),
                Ok(Decision::Ordinary)
            ));
            assert!(decide(&fleet, &args(Some("khion"), false), &env).is_err());
        }
    }

    #[test]
    fn fleet_options_are_taken_only_from_the_default_launch() {
        assert_eq!(
            extract_args(&strings(&["herdr", "--fleet", "lumi", "--new"])),
            Ok((strings(&["herdr"]), args(Some("lumi"), true)))
        );
        assert_eq!(
            extract_args(&strings(&["herdr", "--fleet=lumi", "--help"])),
            Ok((strings(&["herdr", "--help"]), args(Some("lumi"), false)))
        );
        let untouched = strings(&["herdr", "agent", "prompt", "p1", "--new"]);
        assert_eq!(
            extract_args(&untouched),
            Ok((untouched.clone(), FleetLaunchArgs::default()))
        );
        for invalid in [
            &["herdr", "--fleet"][..],
            &["herdr", "--fleet", "-x"],
            &["herdr", "--fleet="],
            &["herdr", "--fleet", "a", "--fleet", "b"],
            &["herdr", "--new", "--new"],
            &["herdr", "--fleet", "lumi", "status"],
            &["herdr", "--new", "server"],
        ] {
            assert!(extract_args(&strings(invalid)).is_err(), "{invalid:?}");
        }
    }

    // Unix path literals; the rule itself is platform-neutral.
    #[cfg(unix)]
    #[test]
    fn plain_launch_and_session_picker_open_the_same_window() {
        let root = Path::new("/state");
        let window = |request: &LaunchRequest| {
            (
                launch::connection_dir(root, &request.config),
                serde_json::to_vec(&request.config).expect("serialize"),
            )
        };
        let origin = Config::from_settings(&settings(), "khion", "khion", "skadi").expect("base");
        let mut picker = State::isolated(origin);
        for (epoch, machine, running) in [(1, "khion", "local"), (2, "lumi", "remote")] {
            picker.apply(Discovery {
                machine: machine.into(),
                epoch,
                result: Ok(vec![DiscoveredSession {
                    name: running.into(),
                    running: true,
                }]),
            });
        }
        let plain = |machine, session: &str, new| {
            launched(decide(&settings(), &args(machine, new), &env(session)))
        };
        let pairs = [
            (
                launch::open(&picker, "local").expect("picker"),
                plain(None, "local", false),
            ),
            (
                launch::open(&picker, "remote").expect("picker"),
                plain(Some("lumi"), "remote", false),
            ),
            (
                launch::create(&picker, "lumi", "fresh").expect("picker"),
                plain(Some("lumi"), "fresh", true),
            ),
            (
                launch::create(&picker, "khion", "fresh").expect("picker"),
                plain(None, "fresh", true),
            ),
        ];
        for (picked, launched) in &pairs {
            assert_eq!(window(picked), window(launched));
            assert_eq!(picked.create_on, launched.create_on);
        }
    }
}
