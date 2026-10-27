use super::*;

fn left_click(state: &mut ClientShellState, rect: Rect) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })])
}

fn sidebar_text(state: &mut ClientShellState) -> String {
    let frame = state.compose(146, 48).unwrap();
    let area = state.layout(146, 48).sidebar;
    fleet_sidebar_text(&frame, area)
}

/// Replaces the local machine's agents and refreshes the fleet projection.
fn set_local_agents(state: &mut ClientShellState, agents: Vec<ClientShellAgent>) {
    let mut snap = snapshot();
    snap.agents = agents;
    state.set_endpoint_snapshot(&ClientEndpointId::Local, Box::new(snap));
    state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Online);
    state.refresh_fleet_agent_visibility();
}

fn local_agent_rect(state: &ClientShellState, pane_id: &str) -> Rect {
    state
        .hits
        .endpoint_agents
        .iter()
        .find(|(_, endpoint, pane)| endpoint.is_local() && pane == pane_id)
        .map(|(rect, _, _)| *rect)
        .expect("visible local agent")
}

#[test]
fn labelled_borders_follow_the_panel_borders_setting() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let bordered = sidebar_text(&mut state);
    assert!(bordered.contains("┌─ spaces ─"), "{bordered}");
    assert!(bordered.contains("├─ agents ─"), "{bordered}");
    assert!(bordered.contains('└'), "{bordered}");

    state.config.panel_borders = false;
    let borderless = sidebar_text(&mut state);
    assert!(!borderless.contains('┌') && !borderless.contains('└'));
    assert!(borderless.contains(" spaces"));
    assert!(borderless.contains(" agents"));
}

#[test]
fn fleet_title_names_the_session_and_scope_label_stays_clickable() {
    let mut state = fleet_state();
    let text = sidebar_text(&mut state);
    assert!(text.contains("┌─ skadi ─"), "{text}");
    assert!(text.contains("─ machines ┐"), "{text}");
    assert!(text.contains("─ this space ┤"), "{text}");
    // The split row is a drag handle everywhere except over its clickable scope label.
    let scope = state.hits.fleet_scope;
    assert!(!scope.is_empty());
    assert_eq!(scope.y, state.hits.sidebar_section_divider.y);
    assert!(!super::super::contains(
        state.hits.sidebar_section_divider,
        (scope.x, scope.y)
    ));
    left_click(&mut state, scope);
    assert_eq!(
        state.fleet.as_ref().unwrap().scope,
        crate::client::fleet::AgentScope::ThisMachine
    );
}

#[test]
fn selected_space_fills_the_whole_row_with_accent_and_ink() {
    let mut state = fleet_tree_state();
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_3");
    let buffer = state.compose(146, 48).unwrap().to_ratatui_buffer().unwrap();
    let materials = crate::ui::Materials::from_palette(&state.config.palette);
    assert_ne!(materials.selected_fg, materials.selected_bg);
    let hit = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.endpoint_id.is_local() && hit.workspace_id == "ws_3")
        .unwrap()
        .rect;
    // From the panel edge, so the tree connector shares the fill.
    for x in state.hits.workspace_body.x..hit.right() {
        let cell = &buffer[(x, hit.y)];
        assert_eq!(cell.bg, materials.selected_bg, "x={x}");
        assert_eq!(cell.fg, materials.selected_fg, "x={x}");
    }
    let focused = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.endpoint_id.is_local() && hit.workspace_id == "ws_1")
        .unwrap()
        .rect;
    assert_eq!(buffer[(focused.x + 1, focused.y)].bg, materials.focused);
}

#[test]
fn detailed_agents_show_model_and_role_and_compact_agents_do_not() {
    let mut state = fleet_state();
    let mut lead = agent("claude", AgentStatus::Working, 1);
    lead.tokens = vec![
        ("model".into(), "opus".into()),
        ("role".into(), "lead".into()),
    ];
    set_local_agents(&mut state, vec![lead]);

    let detailed = sidebar_text(&mut state);
    assert!(detailed.contains("opus · lead"), "{detailed}");
    assert_eq!(local_agent_rect(&state, "pane_1").height, 2);

    state.config.agent_detail = crate::config::AgentDetailConfig::Compact;
    let compact = sidebar_text(&mut state);
    assert!(!compact.contains("opus"), "{compact}");
    assert_eq!(local_agent_rect(&state, "pane_1").height, 1);
}

#[test]
fn agent_path_shows_only_when_it_leaves_the_space_root() {
    let mut state = fleet_state();
    set_local_agents(&mut state, vec![agent("claude", AgentStatus::Idle, 1)]);
    // The fixture pane sits in its space root, and there is no model or role.
    sidebar_text(&mut state);
    assert_eq!(local_agent_rect(&state, "pane_1").height, 1);

    let mut snap = snapshot();
    snap.agents = vec![agent("claude", AgentStatus::Idle, 1)];
    snap.panes[0].foreground_cwd = Some("/home/can/Projects/skadi".into());
    state.set_endpoint_snapshot(&ClientEndpointId::Local, Box::new(snap));
    let text = sidebar_text(&mut state);
    assert_eq!(local_agent_rect(&state, "pane_1").height, 2);
    assert!(
        text.contains("~/Projects/skadi") || text.contains("~/P/skadi"),
        "{text}"
    );
    assert!(!text.contains("/home/can"), "{text}");

    state.config.show_agent_path = false;
    sidebar_text(&mut state);
    assert_eq!(local_agent_rect(&state, "pane_1").height, 1);
}

#[test]
fn agents_group_by_machine_with_blocked_first() {
    let mut state = fleet_state();
    state.fleet.as_mut().unwrap().scope = crate::client::fleet::AgentScope::AllMachines;
    let busy = agent("busy", AgentStatus::Working, 9);
    let mut stuck = agent("stuck", AgentStatus::Blocked, 1);
    stuck.pane_id = "pane_2".into();
    stuck.focused = false;
    set_local_agents(&mut state, vec![busy, stuck]);
    sidebar_text(&mut state);

    let machines = state
        .hits
        .fleet_agent_machines
        .iter()
        .map(|(_, machine)| machine.as_str())
        .collect::<Vec<_>>();
    assert_eq!(machines, ["khion", "lumi"]);
    let order = state
        .hits
        .endpoint_agents
        .iter()
        .map(|(_, endpoint, pane)| (endpoint.is_local(), pane.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        order,
        [(true, "pane_2"), (true, "pane_1"), (false, "pane_1")]
    );
    let khion = state.hits.fleet_agent_machines[0].0;
    assert!(khion.y < local_agent_rect(&state, "pane_2").y);

    // Folding a machine in the agent panel leaves its spaces in the machine tree alone.
    left_click(&mut state, khion);
    assert!(state.fleet_agent_collapsed.contains("khion"));
    assert!(state.fleet.as_ref().unwrap().collapsed.is_empty());
    sidebar_text(&mut state);
    assert!(state
        .hits
        .endpoint_agents
        .iter()
        .all(|(_, endpoint, _)| !endpoint.is_local()));
    assert!(state
        .hits
        .workspaces
        .iter()
        .any(|hit| hit.endpoint_id.is_local()));
}

#[test]
fn footer_actions_route_to_new_space_sessions_and_menu() {
    let mut state = fleet_state();
    state.config.prompt_new_workspace_name = false;
    let text = sidebar_text(&mut state);
    assert!(text.contains("+ space"), "{text}");
    assert!(text.contains("sessions"), "{text}");
    assert!(text.contains('≡'), "{text}");
    let new_space = state.hits.new_workspace;
    let sessions = state.hits.session_picker;
    let menu = state.hits.global_launcher;
    assert!(!new_space.is_empty() && !sessions.is_empty() && !menu.is_empty());
    assert!(new_space.right() <= sessions.x && sessions.right() <= menu.x);
    assert_eq!(new_space.y, state.layout(146, 48).sidebar.bottom() - 2);

    let created = left_click(&mut state, new_space);
    assert!(created.actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(request.method, crate::api::schema::Method::WorkspaceCreate(_))
    )));

    let picked = left_click(&mut state, sessions);
    assert!(picked.repaint);
    assert!(picked.actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::SessionPicker(_))
    ));
    state.handle_input_bytes(b"\x1b");
    assert!(state.overlay.is_none());

    left_click(&mut state, menu);
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::GlobalMenu(_))
    ));
}

#[test]
fn ordinary_windows_have_no_session_picker() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let text = sidebar_text(&mut state);
    assert!(
        text.contains("+ space") && text.contains("≡ menu"),
        "{text}"
    );
    assert!(!text.contains("sessions"), "{text}");
    assert!(state.hits.session_picker.is_empty());
    assert!(!state.hits.new_workspace.is_empty());
}
