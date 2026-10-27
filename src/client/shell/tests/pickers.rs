use super::*;

fn agent(
    pane_id: &str,
    kind: &str,
    status: AgentStatus,
    tokens: &[(&str, &str)],
) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: pane_id.into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: None,
        display_agent: None,
        agent: Some(kind.into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: status,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: tokens
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
        focused: false,
    }
}

fn local_state(agents: Vec<ClientShellAgent>) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut snapshot = snapshot();
    snapshot.agents = agents;
    state.set_snapshot(Box::new(snapshot));
    state.set_pane_surface(surface());
    state
}

fn discovery(
    machine: &str,
    epoch: u64,
    sessions: &[(&str, bool)],
) -> crate::client::fleet::Discovery {
    crate::client::fleet::Discovery {
        machine: machine.into(),
        epoch,
        result: Ok(sessions
            .iter()
            .map(|(name, running)| crate::client::fleet::DiscoveredSession {
                name: (*name).into(),
                running: *running,
            })
            .collect()),
    }
}

/// A combined skadi window across khion (local) and lumi, where both machines also report
/// unrelated sessions.
fn fleet_state() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut fleet = crate::client::fleet::State::isolated(crate::client::fleet::tests::config());
    fleet.apply(discovery(
        "khion",
        1,
        &[("skadi", true), ("default", true), ("odyssey", false)],
    ));
    fleet.apply(discovery("lumi", 1, &[("skadi", true), ("research", true)]));
    state.set_fleet(fleet);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let lumi = state
        .fleet
        .as_ref()
        .expect("fleet")
        .config
        .endpoint_id("lumi", "skadi");
    let mut remote = snapshot();
    remote.boot_id = "lumi-boot".into();
    remote.workspaces[0].label = "build".into();
    remote.agents = vec![agent(
        "pane_9",
        "codex",
        AgentStatus::Blocked,
        &[("role", "implementer"), ("model", "sol")],
    )];
    state.set_endpoint_snapshot(&lumi, Box::new(remote));
    state.set_endpoint_status(&lumi, ClientEndpointStatus::Online);
    state
}

fn role_rows(state: &ClientShellState) -> Vec<ClientNavigatorRow> {
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!("expected the role picker");
    };
    render::client_navigator_rows(&state.endpoints, &state.active_endpoint_id, navigator)
}

fn headings(rows: &[ClientNavigatorRow]) -> Vec<&str> {
    rows.iter()
        .filter(|row| matches!(row.target, ClientNavigatorTarget::RoleGroup { .. }))
        .map(|row| row.label.as_str())
        .collect()
}

fn selected_row(state: &ClientShellState) -> ClientNavigatorRow {
    let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_ref() else {
        panic!("expected the role picker");
    };
    let rows = role_rows(state);
    let index =
        aggregate_navigation::navigator_selected_index(&rows, navigator).expect("selected row");
    rows[index].clone()
}

fn group_of(rows: &[ClientNavigatorRow], target: &ClientNavigatorTarget) -> String {
    let index = rows
        .iter()
        .position(|row| &row.target == target)
        .expect("row");
    rows[..index]
        .iter()
        .rev()
        .find(|row| matches!(row.target, ClientNavigatorTarget::RoleGroup { .. }))
        .map(|row| row.label.clone())
        .expect("group heading")
}

fn grouped_agents() -> Vec<ClientShellAgent> {
    vec![
        agent("pane_1", "pi", AgentStatus::Idle, &[]),
        agent(
            "pane_2",
            "claude",
            AgentStatus::Working,
            &[("role", "zeta")],
        ),
        agent(
            "pane_3",
            "codex",
            AgentStatus::Working,
            &[("role", "implementer"), ("model", "sol")],
        ),
        agent("pane_4", "claude", AgentStatus::Done, &[("role", "alpha")]),
        agent(
            "pane_5",
            "claude",
            AgentStatus::Blocked,
            &[("role", "implementer"), ("model", "sonnet")],
        ),
        agent(
            "pane_6",
            "claude",
            AgentStatus::Idle,
            &[("role", "Orchestrator"), ("model", "opus")],
        ),
        agent("pane_7", "pi", AgentStatus::Idle, &[("role", "  ")]),
    ]
}

#[test]
fn role_picker_orders_configured_roles_then_reported_roles_then_unassigned() {
    let mut state = local_state(grouped_agents());
    state.open_role_picker();
    let rows = role_rows(&state);

    // Configured order is orchestrator, architect, implementer; architect has no agents.
    assert_eq!(
        headings(&rows),
        vec!["orchestrator", "implementer", "alpha", "zeta", "unassigned"]
    );
    let implementers = rows
        .iter()
        .skip_while(|row| row.label != "implementer")
        .skip(1)
        .take_while(|row| row.status.is_some())
        .map(|row| row.meta.as_str())
        .collect::<Vec<_>>();
    assert_eq!(implementers, vec!["sonnet", "sol"], "blocked sorts first");
    let unassigned = rows
        .iter()
        .skip_while(|row| row.label != "unassigned")
        .skip(1)
        .count();
    assert_eq!(unassigned, 2, "missing and blank roles are both unassigned");
}

#[test]
fn role_picker_filter_matches_every_column_and_never_selects_a_heading() {
    let mut state = local_state(grouped_agents());
    state.open_role_picker();
    for (query, expected) in [
        ("sonnet", vec!["pane_5"]),
        ("orch", vec!["pane_6"]),
        ("client-shell zeta", vec!["pane_2"]),
        ("local codex", vec!["pane_3"]),
        ("nothing-matches", vec![]),
    ] {
        if let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() {
            navigator.query.clear();
            navigator.query.insert(query);
            navigator.selected = Some(ClientNavigatorTarget::Pane {
                endpoint_id: ClientEndpointId::Local,
                pane_id: "pane_1".into(),
            });
        }
        let rows = role_rows(&state);
        let panes = rows
            .iter()
            .filter_map(|row| match &row.target {
                ClientNavigatorTarget::Pane { pane_id, .. } => Some(pane_id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(panes, expected, "{query}");
        if !expected.is_empty() {
            // The previous selection was filtered away; it must fall to an agent row.
            assert!(selected_row(&state).status.is_some(), "{query}");
        }
    }
}

#[test]
fn role_picker_typing_filters_and_tab_and_digits_jump_between_groups() {
    let mut state = local_state(grouped_agents());
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::OpenRolePicker),
        &mut ClientShellInput::default(),
    );
    let rows = role_rows(&state);
    let group = |state: &ClientShellState| group_of(&rows, &selected_row(state).target);
    assert_eq!(group(&state), "unassigned", "opens on the focused pane");

    state.handle_input_bytes(b"\t");
    assert_eq!(group(&state), "orchestrator", "tab wraps around");
    state.handle_input_bytes(b"\t");
    assert_eq!(group(&state), "implementer");
    state.handle_input_bytes(b"\t");
    assert_eq!(group(&state), "alpha");
    state.handle_input_bytes(b"\x1b[Z");
    assert_eq!(group(&state), "implementer");
    state.handle_input_bytes(b"5");
    assert_eq!(group(&state), "unassigned");
    state.handle_input_bytes(b"9");
    assert_eq!(group(&state), "unassigned", "missing groups are ignored");
    state.handle_input_bytes(b"1");
    assert_eq!(group(&state), "orchestrator");

    // Moving up from the first agent cannot land on the heading above it.
    state.handle_input_bytes(b"\x1b[A");
    assert!(selected_row(&state).status.is_some());
    state.handle_input_bytes(b"\x1b[B");
    assert_eq!(group(&state), "implementer", "down skips the next heading");

    state.handle_input_bytes(b"sonnet");
    let rows = role_rows(&state);
    assert_eq!(rows.len(), 2);
    assert!(state.overlay.is_some());
    state.handle_input_bytes(b"\x1b");
    assert!(state.overlay.is_none());
}

#[test]
fn role_picker_enter_focuses_the_agent_pane_on_its_endpoint() {
    let mut state = fleet_state();
    state.open_role_picker();
    let lumi = state
        .fleet
        .as_ref()
        .expect("fleet")
        .config
        .endpoint_id("lumi", "skadi");
    let rows = role_rows(&state);
    let row = rows
        .iter()
        .find(|row| row.label == "codex")
        .expect("lumi agent");
    assert_eq!(row.machine, "lumi", "fleet rows name the machine");
    assert_eq!(row.detail, "build");

    state.handle_input_bytes(b"lumi");
    let outcome = state.handle_input_bytes(b"\r");

    assert!(matches!(
        outcome.actions.as_slice(),
        [ClientShellAction::ActivateEndpoint {
            endpoint_id,
            target: Some(ClientEndpointFocusTarget::Pane(pane_id)),
        }] if endpoint_id == &lumi && pane_id == "pane_9"
    ));
    assert!(state.overlay.is_none());
}

#[test]
fn role_picker_renders_headings_without_hit_regions() {
    let mut state = local_state(grouped_agents());
    state.open_role_picker();
    let frame = state.compose(100, 30).expect("role picker frame");
    let text = frame_rows(&frame).join("\n");
    assert!(text.contains("agents by role"));
    assert!(text.contains("implementer"));
    assert_eq!(state.hits.navigator_rows.len(), grouped_agents().len());
    assert!(state
        .hits
        .navigator_rows
        .iter()
        .all(|(_, target)| matches!(target, ClientNavigatorTarget::Pane { .. })));
}

#[test]
fn session_picker_lists_discovered_sessions_without_attaching_them() {
    let mut state = fleet_state();
    let endpoints_before = state
        .endpoints
        .iter()
        .map(|endpoint| endpoint.endpoint_id.clone())
        .collect::<Vec<_>>();
    let fleet = state.fleet.as_ref().expect("fleet");
    assert!(fleet
        .sessions
        .values()
        .all(|session| session.name == "skadi"));
    assert_eq!(endpoints_before.len(), 2);

    let rows = super::super::pickers::session_rows(fleet);
    let summary = rows
        .iter()
        .map(|row| (row.name.as_str(), row.machines.clone(), row.current))
        .collect::<Vec<_>>();
    assert_eq!(
        summary,
        vec![
            ("default", vec![Some(true), None], false),
            ("odyssey", vec![Some(false), None], false),
            ("research", vec![None, Some(true)], false),
            ("skadi", vec![Some(true), Some(true)], true),
        ]
    );

    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::OpenSessionPicker),
        &mut ClientShellInput::default(),
    );
    let frame = state.compose(100, 30).expect("session picker frame");
    assert!(frame_rows(&frame).join("\n").contains("sessions"));
    assert_eq!(state.hits.session_rows.len(), 4);

    state.handle_input_bytes(b"k");
    let outcome = state.handle_input_bytes(b"\r");
    assert!(outcome.actions.is_empty());
    assert!(state.overlay.is_none());
    let launch = outcome.launch.expect("relaunch request");
    assert_eq!(launch.config.origin_session, "research");
    assert_eq!(
        launch.config.selected.machine, "lumi",
        "research only runs on lumi"
    );
    assert!(!launch.attach_only && launch.create_on.is_none());
    assert_eq!(
        state
            .endpoints
            .iter()
            .map(|endpoint| endpoint.endpoint_id.clone())
            .collect::<Vec<_>>(),
        endpoints_before,
        "choosing another session never registers its endpoints"
    );
}

fn open_picker_on(state: &mut ClientShellState, session: &str) {
    state.open_session_picker();
    if let Some(ClientShellOverlay::SessionPicker(picker)) = state.overlay.as_mut() {
        picker.selected = Some(session.into());
    }
}

#[test]
fn session_picker_enter_relaunches_locally_attached_only_when_local_runs_it() {
    let mut state = fleet_state();
    open_picker_on(&mut state, "default");
    let launch = state.handle_input_bytes(b"\r").launch.expect("default");
    assert_eq!(launch.config.selected.machine, "khion");
    assert!(launch.attach_only, "running locally attaches with client");

    open_picker_on(&mut state, "odyssey");
    let launch = state.handle_input_bytes(b"\r").launch.expect("odyssey");
    assert_eq!(launch.config.selected.machine, "khion");
    assert!(
        !launch.attach_only,
        "a stopped local session starts with the window"
    );

    open_picker_on(&mut state, "skadi");
    let outcome = state.handle_input_bytes(b"\r");
    assert!(outcome.launch.is_none(), "the current session just closes");
    assert!(state.overlay.is_none());
}

#[test]
fn session_picker_refuses_a_session_stopped_only_on_remote_machines() {
    let mut state = fleet_state();
    state.fleet.as_mut().expect("fleet").apply(discovery(
        "lumi",
        2,
        &[("skadi", true), ("parked", false)],
    ));
    open_picker_on(&mut state, "parked");
    let outcome = state.handle_input_bytes(b"\r");
    assert!(outcome.launch.is_none());
    assert!(state
        .endpoint_error
        .as_deref()
        .is_some_and(|notice| notice.contains("parked")));
}

fn new_session_prompt(state: &ClientShellState) -> (&str, &str) {
    match state.overlay.as_ref() {
        Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            input,
            target: ClientRenameTarget::NewSession { machine },
            ..
        })) => (input.as_str(), machine.as_str()),
        _ => panic!("expected the new session prompt"),
    }
}

#[test]
fn session_picker_n_prompts_for_a_validated_name_and_tab_cycles_machines() {
    let mut state = fleet_state();
    state.open_session_picker();
    state.handle_input_bytes(b"n");
    assert_eq!(new_session_prompt(&state), ("", "khion"));
    let frame = state.compose(100, 30).expect("prompt frame");
    assert!(frame_rows(&frame).join("\n").contains("on khion"));

    state.handle_input_bytes(b"\t");
    assert_eq!(new_session_prompt(&state).1, "lumi");
    state.handle_input_bytes(b"\t");
    assert_eq!(new_session_prompt(&state).1, "khion", "tab wraps");
    state.handle_input_bytes(b"\x1b[Z");
    assert_eq!(new_session_prompt(&state).1, "lumi", "shift-tab reverses");

    for rejected in ["default", "bad/name", "research"] {
        if let Some(ClientShellOverlay::Rename(rename)) = state.overlay.as_mut() {
            rename.input.clear();
            rename.input.insert(rejected);
        }
        let outcome = state.handle_input_bytes(b"\r");
        assert!(outcome.launch.is_none(), "{rejected}");
        assert_eq!(new_session_prompt(&state).0, rejected, "prompt stays open");
    }
    let frame = state.compose(100, 30).expect("prompt frame");
    assert!(frame_rows(&frame)
        .join("\n")
        .contains("already exists on lumi"));

    if let Some(ClientShellOverlay::Rename(rename)) = state.overlay.as_mut() {
        rename.input.clear();
    }
    state.handle_input_bytes(b"fresh");
    let outcome = state.handle_input_bytes(b"\r");
    assert!(state.overlay.is_none());
    let launch = outcome.launch.expect("create request");
    assert_eq!(launch.config.origin_session, "fresh");
    assert_eq!(launch.config.selected.machine, "lumi");
    assert_eq!(launch.create_on.as_deref(), Some("feltfomo@lumi"));
    assert!(!launch.attach_only);
}

#[test]
fn session_picker_footer_names_the_new_session_key() {
    let mut state = fleet_state();
    state.open_session_picker();
    let frame = state.compose(100, 30).expect("session picker frame");
    assert!(frame_rows(&frame).join("\n").contains("n new"));
}

#[test]
fn session_listing_survives_discovery_failure_and_stays_out_of_endpoints() {
    let mut fleet = crate::client::fleet::State::isolated(crate::client::fleet::tests::config());
    fleet.apply(discovery("lumi", 1, &[("odyssey", true)]));
    fleet.apply(crate::client::fleet::Discovery {
        machine: "lumi".into(),
        epoch: 2,
        result: Err("offline".into()),
    });
    let rows = super::super::pickers::session_rows(&fleet);
    assert_eq!(
        rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
        vec!["odyssey", "skadi"]
    );
    assert_eq!(rows[1].machines, vec![Some(true), None], "origin is local");
    assert_eq!(fleet.sessions.len(), 1);
}

#[test]
fn session_picker_does_nothing_outside_combined_windows() {
    let mut state = local_state(Vec::new());
    state.open_session_picker();
    assert!(state.overlay.is_none());
}
