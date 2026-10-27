use super::*;

pub(super) fn watch_profiles(
    event_tx: tokio::sync::mpsc::Sender<ClientLoopEvent>,
    should_quit: Arc<AtomicBool>,
) {
    // One bounded read per second per client, independent of rendering and pane count.
    std::thread::spawn(move || {
        let mut previous = None;
        while !should_quit.load(Ordering::Acquire) {
            let current = endpoint::EndpointCatalog::load_profiles();
            if previous.as_ref() != Some(&current) {
                previous = Some(current.clone());
                if event_tx
                    .blocking_send(ClientLoopEvent::EndpointCatalog(current))
                    .is_err()
                {
                    break;
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

// Only called between surface handoffs: removing a source must not invalidate an in-flight
// rollback. Connection attempts are independent and fenced by supervisor generations.
pub(super) fn apply_profiles(
    state: &mut ClientState,
    endpoints: &mut endpoint::EndpointRegistry,
    commands: &mut endpoint_commands::EndpointCommands,
    supervisors: &mut endpoint::EndpointSupervisors,
    catalog: &mut endpoint::EndpointCatalog,
    profiles: Vec<endpoint::SavedSshEndpoint>,
    now: std::time::Instant,
) -> bool {
    if catalog.ssh == profiles {
        return false;
    }
    let previous_size = state
        .shell
        .as_ref()
        .map(|shell| shell.surface_size(state.reported_size.0, state.reported_size.1));
    let retired = supervisors.reconcile_profiles(&profiles, now);
    let active_removed = retired.contains(endpoints.active_id());
    for endpoint_id in retired {
        endpoints.disconnect(&endpoint_id);
        let cancelled = commands.disconnect(&endpoint_id);
        #[cfg(unix)]
        state.forget_endpoint_graphics(&endpoint_id);
        if let Some(shell) = state.shell.as_mut() {
            for request_id in cancelled {
                shell.cancel_endpoint_request(&request_id);
            }
            shell.retire_endpoint(&endpoint_id);
        }
    }
    catalog.ssh = profiles;
    if active_removed {
        endpoints.select_unavailable_local();
        catalog.select_local();
        state.freeze_presentation();
        if let Some(shell) = state.shell.as_mut() {
            shell.select_unavailable_local();
        }
    } else if catalog.selected_profile.as_ref().is_some_and(|selected| {
        !catalog
            .ssh
            .iter()
            .any(|profile| &profile.id == selected && profile.enabled)
    }) {
        catalog.select_local();
    }
    let pixel_mouse_available = state.pixel_mouse_available();
    if let Some(shell) = state.shell.as_mut() {
        shell.set_endpoint_catalog(&catalog.ssh);
        if endpoints.active_surface_available()
            && previous_size
                != Some(shell.surface_size(state.reported_size.0, state.reported_size.1))
        {
            shell.invalidate_pane_surface();
            endpoints.send(&client_shell_resize_message(
                shell,
                state.reported_size.0,
                state.reported_size.1,
                state.reported_cell_size.0,
                state.reported_cell_size.1,
                pixel_mouse_available,
            ));
        }
    }
    active_removed
}

pub(super) fn apply_fleet(
    state: &mut ClientState,
    endpoints: &mut endpoint::EndpointRegistry,
    commands: &mut endpoint_commands::EndpointCommands,
    supervisors: &mut endpoint::EndpointSupervisors,
    discovery: fleet::Discovery,
    now: std::time::Instant,
) -> Option<bool> {
    let shell = state.shell.as_mut()?;
    let fleet = shell.fleet.as_mut()?;
    if !fleet.apply(discovery) {
        return None;
    }
    let retired = supervisors.reconcile_fleet(&fleet.sessions, now);
    let active_removed = retired.contains(endpoints.active_id());
    for endpoint_id in retired {
        endpoints.disconnect(&endpoint_id);
        let cancelled = commands.disconnect(&endpoint_id);
        #[cfg(unix)]
        state.forget_endpoint_graphics(&endpoint_id);
        if let Some(shell) = state.shell.as_mut() {
            for request_id in cancelled {
                shell.cancel_endpoint_request(&request_id);
            }
            shell.retire_endpoint(&endpoint_id);
        }
    }
    if active_removed {
        endpoints.select_unavailable_local();
        state.freeze_presentation();
        if let Some(shell) = state.shell.as_mut() {
            shell.select_unavailable_local();
            if let Some(fleet) = shell.fleet.as_mut() {
                fleet.selected = fleet::Selection {
                    machine: fleet.config.local_machine.clone(),
                    session: fleet.config.origin_session.clone(),
                };
                fleet.persist();
            }
        }
    }
    if let Some(shell) = state.shell.as_mut() {
        shell.refresh_fleet_endpoints();
    }
    Some(active_removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use endpoint::{ClientEndpointId, EndpointCatalog, EndpointRegistry, EndpointSupervisors};
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    struct Transport(Arc<AtomicUsize>);

    impl endpoint::EndpointTransport for Transport {
        fn send(&mut self, _: &ClientMessage) -> io::Result<()> {
            Ok(())
        }

        fn disconnect(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn state() -> ClientState {
        ClientState::test_new()
    }

    #[test]
    fn fleet_active_session_retirement_requests_origin_recovery() {
        let now = Instant::now();
        let mut state = state();
        let mut fleet = fleet::State::isolated(fleet::tests::config());
        let discovery = |epoch, sessions| fleet::Discovery {
            machine: "lumi".into(),
            epoch,
            result: Ok(sessions),
        };
        fleet.apply(discovery(
            1,
            vec![fleet::DiscoveredSession {
                name: "skadi".into(),
                running: true,
            }],
        ));
        let remote = fleet.config.endpoint_id("lumi", "skadi");
        let mut supervisors = EndpointSupervisors::new(&[], now);
        supervisors.reconcile_fleet(&fleet.sessions, now);
        state.shell.as_mut().unwrap().set_fleet(fleet);
        let mut endpoints = EndpointRegistry::new(
            Transport(Arc::new(AtomicUsize::new(0))),
            1,
            Default::default(),
        );
        endpoints.insert(
            remote.clone(),
            Transport(Arc::new(AtomicUsize::new(0))),
            2,
            Default::default(),
            true,
        );
        endpoints.set_active(&remote);
        endpoints.unfreeze_input();
        let mut commands = endpoint_commands::EndpointCommands::default();
        assert_eq!(
            apply_fleet(
                &mut state,
                &mut endpoints,
                &mut commands,
                &mut supervisors,
                discovery(2, Vec::new()),
                now
            ),
            Some(true)
        );
        assert_eq!(endpoints.active_id(), &ClientEndpointId::Local);
        assert!(endpoints.connection(&ClientEndpointId::Local).is_some());
        assert!(endpoints.connection(&remote).is_none());
        let shell = state.shell.as_mut().unwrap();
        let fleet = shell.fleet.as_ref().unwrap();
        assert_eq!(fleet.selected.machine, "khion");
        assert_eq!(fleet.selected.session, "skadi");
        assert!(shell.compose(120, 40).is_some());
    }

    #[test]
    fn live_catalog_add_and_rename_keep_local_connection_and_selection() {
        let now = Instant::now();
        let mut state = state();
        let disconnected = Arc::new(AtomicUsize::new(0));
        let mut endpoints =
            EndpointRegistry::new(Transport(disconnected.clone()), 1, Default::default());
        let mut supervisors = EndpointSupervisors::new(&[], now);
        let mut commands = endpoint_commands::EndpointCommands::default();
        let mut catalog = EndpointCatalog::default();
        let mut profile = endpoint::SavedSshEndpoint::new("Build", "build", "main").unwrap();
        for label in ["Build", "Renamed"] {
            profile.label = label.into();
            assert!(!apply_profiles(
                &mut state,
                &mut endpoints,
                &mut commands,
                &mut supervisors,
                &mut catalog,
                vec![profile.clone()],
                now
            ));
            assert_eq!(endpoints.active_id(), &ClientEndpointId::Local);
            assert!(endpoints.active_surface_available());
            assert_eq!(
                endpoints
                    .connection(&ClientEndpointId::Local)
                    .unwrap()
                    .generation,
                1
            );
            assert_eq!(catalog.selected_profile, None);
            assert_eq!(
                state
                    .shell
                    .as_ref()
                    .unwrap()
                    .endpoint_label(&ClientEndpointId::Ssh(profile.id.clone())),
                label
            );
        }
        assert_eq!(disconnected.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn live_catalog_remove_or_disable_active_machine_selects_local_without_input() {
        for local_online in [false, true] {
            for disable in [false, true] {
                let now = Instant::now();
                let mut state = state();
                let mut catalog = EndpointCatalog::default();
                let id = catalog.add_ssh("Build", "build", "main").unwrap();
                let remote = ClientEndpointId::Ssh(id.clone());
                catalog.select_ssh(&id);
                state
                    .shell
                    .as_mut()
                    .unwrap()
                    .set_endpoint_catalog(&catalog.ssh);
                let mut supervisors = EndpointSupervisors::new(&catalog.ssh, now);
                let mut endpoints = EndpointRegistry::empty();
                let local_disconnects = Arc::new(AtomicUsize::new(0));
                if local_online {
                    endpoints.insert(
                        ClientEndpointId::Local,
                        Transport(local_disconnects.clone()),
                        1,
                        Default::default(),
                        false,
                    );
                }
                let remote_disconnects = Arc::new(AtomicUsize::new(0));
                endpoints.insert(
                    remote.clone(),
                    Transport(remote_disconnects.clone()),
                    2,
                    Default::default(),
                    true,
                );
                endpoints.set_active(&remote);
                endpoints.unfreeze_input();
                let mut commands = endpoint_commands::EndpointCommands::default();
                let profiles = if disable {
                    let mut profiles = catalog.ssh.clone();
                    profiles[0].enabled = false;
                    profiles
                } else {
                    Vec::new()
                };
                assert!(apply_profiles(
                    &mut state,
                    &mut endpoints,
                    &mut commands,
                    &mut supervisors,
                    &mut catalog,
                    profiles,
                    now
                ));
                assert_eq!(endpoints.active_id(), &ClientEndpointId::Local);
                assert!(!endpoints.active_surface_available());
                assert!(endpoints.connection(&remote).is_none());
                assert_eq!(
                    endpoints.connection(&ClientEndpointId::Local).is_some(),
                    local_online
                );
                assert!(state
                    .shell
                    .as_ref()
                    .unwrap()
                    .endpoint_is_active(&ClientEndpointId::Local));
                assert!(!state.shell.as_ref().unwrap().has_presented_surface());
                assert!(state.presentation_frozen);
                assert_eq!(catalog.selected_profile, None);
                assert_eq!(remote_disconnects.load(Ordering::Relaxed), 1);
                assert_eq!(local_disconnects.load(Ordering::Relaxed), 0);
                assert!(!supervisors.record_status(
                    &remote,
                    2,
                    endpoint::ClientEndpointStatus::Online,
                    now
                ));
            }
        }
    }
}
