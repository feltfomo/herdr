//! Remote-host side of the SSH stdio bridge.

use std::io;
use std::time::Duration;

use super::RemoteAttachPolicy;

pub(crate) fn run_remote_client_bridge(args: &[String]) -> io::Result<()> {
    let (idle_timeout, policy) =
        parse_bridge_options(args, crate::platform::REMOTE_BRIDGE_IDLE_TIMEOUT_SUPPORTED)?;
    ensure_remote_server_running(policy)?;
    #[cfg(unix)]
    let _ssh_agent = super::ssh_agent::Registration::start();

    let socket_path = crate::server::socket_paths::client_socket_path();
    let stream = crate::ipc::connect_local_stream(&socket_path).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "failed to connect to remote Herdr client socket {}: {err}",
                socket_path.display()
            ),
        )
    })?;

    crate::platform::forward_remote_bridge_stdio(stream, idle_timeout)
}

fn parse_bridge_options(
    args: &[String],
    idle_supported: bool,
) -> io::Result<(bool, RemoteAttachPolicy)> {
    let mut idle_timeout = false;
    let mut policy = RemoteAttachPolicy::StartIfMissing;
    for option in args {
        match option.as_str() {
            "--idle-timeout-v1" if idle_supported && !idle_timeout => idle_timeout = true,
            "--existing-only-v1" if policy == RemoteAttachPolicy::StartIfMissing => {
                policy = RemoteAttachPolicy::ExistingOnly;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unsupported remote client bridge option",
                ))
            }
        }
    }
    Ok((idle_timeout, policy))
}

fn ensure_remote_server_running(policy: RemoteAttachPolicy) -> io::Result<()> {
    let socket_path = crate::server::socket_paths::client_socket_path();
    ensure_remote_server_with(
        policy,
        || {
            if !crate::server::autodetect::is_server_listening()? {
                return Ok(false);
            }
            let status = crate::api::read_runtime_status_at(
                &crate::api::socket_path(),
                Duration::from_millis(500),
            )?
            .ok_or_else(|| io::Error::other("remote server status API is unavailable"))?;
            if status
                .capabilities
                .as_ref()
                .and_then(|capabilities| capabilities.endpoint_protocol_generation)
                == Some(crate::protocol::endpoint::ENDPOINT_PROTOCOL_GENERATION)
            {
                return Ok(true);
            }
            Err(io::Error::other(
            "remote herdr server needs one final update before this bridge can attach; rerun `herdr --remote` from an interactive terminal to approve it",
        ))
        },
        || {
            crate::server::autodetect::spawn_server_daemon()?;
            crate::server::autodetect::wait_for_server_socket(&socket_path, Duration::from_secs(5))
        },
    )
}

fn ensure_remote_server_with(
    policy: RemoteAttachPolicy,
    inspect: impl FnOnce() -> io::Result<bool>,
    start: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    if inspect()? {
        return Ok(());
    }
    match policy {
        RemoteAttachPolicy::ExistingOnly => Err(io::Error::new(io::ErrorKind::NotFound,
            "remote Herdr session is stopped; attach or create it explicitly on the peer to start it")),
        RemoteAttachPolicy::StartIfMissing => start(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_options_combine_independently_and_reject_unknown_or_duplicate_flags() {
        for args in [
            vec!["--existing-only-v1", "--idle-timeout-v1"],
            vec!["--idle-timeout-v1", "--existing-only-v1"],
        ] {
            let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert_eq!(
                parse_bridge_options(&args, true).unwrap(),
                (true, RemoteAttachPolicy::ExistingOnly)
            );
        }
        assert_eq!(
            parse_bridge_options(&[], false).unwrap(),
            (false, RemoteAttachPolicy::StartIfMissing)
        );
        assert_eq!(
            parse_bridge_options(&["--existing-only-v1".into()], false).unwrap(),
            (false, RemoteAttachPolicy::ExistingOnly)
        );
        for args in [
            vec!["--existing-only-v1", "--existing-only-v1"],
            vec!["--idle-timeout-v1", "--idle-timeout-v1"],
            vec!["--unknown"],
        ] {
            let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert!(parse_bridge_options(&args, true).is_err());
        }
        assert!(parse_bridge_options(&["--idle-timeout-v1".into()], false).is_err());
    }

    #[test]
    fn existing_only_missing_server_never_invokes_startup() {
        let error = ensure_remote_server_with(
            RemoteAttachPolicy::ExistingOnly,
            || Ok(false),
            || panic!("existing-only attachment must never spawn a server"),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn ordinary_bridge_starts_a_missing_server() {
        let started = std::cell::Cell::new(false);
        ensure_remote_server_with(
            RemoteAttachPolicy::StartIfMissing,
            || Ok(false),
            || {
                started.set(true);
                Ok(())
            },
        )
        .unwrap();
        assert!(started.get());
    }

    #[test]
    fn listening_or_incompatible_servers_are_never_restarted() {
        for policy in [
            RemoteAttachPolicy::ExistingOnly,
            RemoteAttachPolicy::StartIfMissing,
        ] {
            ensure_remote_server_with(
                policy,
                || Ok(true),
                || panic!("running server was restarted"),
            )
            .unwrap();
            let error = ensure_remote_server_with(
                policy,
                || {
                    Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "incompatible server",
                    ))
                },
                || panic!("incompatible server was restarted"),
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        }
    }
}
