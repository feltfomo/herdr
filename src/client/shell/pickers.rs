//! Floating pickers for combined windows: agents grouped by role (a navigator mode) and the
//! logical sessions the fleet discovered.

use super::*;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

const ROLE_TOKEN: &str = "role";
const MODEL_TOKEN: &str = "model";
const UNASSIGNED_ROLE: &str = "unassigned";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RoleGroupKey {
    Configured(usize),
    Reported(String),
    Unassigned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RoleGroupJump {
    Next,
    Previous,
    Index(usize),
}

/// Agents from every endpoint with a snapshot, grouped by their `role` token: configured
/// roles in configured order, other reported roles alphabetically, then agents with no role.
///
/// The sidebar's agent scope is deliberately ignored here. The picker is the way to reach an
/// agent the sidebar is currently scoped away from.
pub(super) fn role_rows(
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    navigator: &ClientNavigatorOverlay,
    order: &[String],
) -> Vec<ClientNavigatorRow> {
    let query = navigator.query.trim().to_lowercase();
    let words = query.split_whitespace().collect::<Vec<_>>();
    let mut groups =
        std::collections::BTreeMap::<RoleGroupKey, (String, Vec<(_, ClientNavigatorRow)>)>::new();
    for endpoint in super::aggregate_navigation::cached_endpoint_snapshots(endpoints) {
        let machine = endpoints[endpoint.endpoint_index].machine_label();
        let snapshot = endpoint.snapshot;
        for agent in &snapshot.agents {
            let token = |name: &str| {
                agent
                    .tokens
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.trim())
                    .filter(|value| !value.is_empty())
            };
            let (key, group_label) = match token(ROLE_TOKEN) {
                Some(role) => match order
                    .iter()
                    .position(|configured| configured.eq_ignore_ascii_case(role))
                {
                    Some(index) => (RoleGroupKey::Configured(index), order[index].as_str()),
                    None => (RoleGroupKey::Reported(role.to_owned()), role),
                },
                None => (RoleGroupKey::Unassigned, UNASSIGNED_ROLE),
            };
            let kind = agent
                .agent
                .as_deref()
                .or(agent.display_agent.as_deref())
                .or(agent.name.as_deref())
                .unwrap_or("agent");
            let space = snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == agent.workspace_id)
                .map_or(agent.workspace_id.as_str(), |workspace| {
                    workspace.label.as_str()
                });
            let model = token(MODEL_TOKEN).unwrap_or_default();
            if !words.is_empty() {
                let haystack =
                    format!("{group_label} {kind} {space} {machine} {model}").to_lowercase();
                if !words.iter().all(|word| haystack.contains(word)) {
                    continue;
                }
            }
            let stale = endpoint.stale();
            let urgency = (
                stale,
                std::cmp::Reverse(status_priority(agent.agent_status)),
                std::cmp::Reverse(
                    endpoint
                        .agent_recency
                        .get(&agent.pane_id)
                        .copied()
                        .unwrap_or_default(),
                ),
            );
            let row = ClientNavigatorRow {
                depth: 1,
                label: kind.to_owned(),
                meta: model.to_owned(),
                detail: space.to_owned(),
                machine: machine.to_owned(),
                agent: Some(kind.to_owned()),
                status: Some(agent.agent_status),
                stale,
                current: endpoint.endpoint_id == active_endpoint_id
                    && snapshot.focused_pane_id.as_deref() == Some(agent.pane_id.as_str()),
                target: ClientNavigatorTarget::Pane {
                    endpoint_id: endpoint.endpoint_id.clone(),
                    pane_id: agent.pane_id.clone(),
                },
            };
            groups
                .entry(key)
                .or_insert_with(|| (group_label.to_owned(), Vec::new()))
                .1
                .push((urgency, row));
        }
    }
    let mut rows = Vec::new();
    for (key, (label, mut members)) in groups {
        // Stable, so equally urgent agents keep endpoint and snapshot order.
        members.sort_by_key(|member| member.0);
        rows.push(ClientNavigatorRow {
            depth: 0,
            label: label.clone(),
            meta: String::new(),
            detail: String::new(),
            machine: String::new(),
            agent: None,
            status: None,
            stale: false,
            current: false,
            target: ClientNavigatorTarget::RoleGroup {
                role: (key != RoleGroupKey::Unassigned).then_some(label),
            },
        });
        rows.extend(members.into_iter().map(|(_, row)| row));
    }
    rows
}

pub(super) fn is_role_group(row: &ClientNavigatorRow) -> bool {
    matches!(row.target, ClientNavigatorTarget::RoleGroup { .. })
}

/// The closest agent row to `index`, preferring the direction of travel. Role headings are
/// never selected, so a move or scroll that lands on one continues past it.
pub(super) fn selectable_row(
    rows: &[ClientNavigatorRow],
    index: usize,
    forward: bool,
) -> Option<usize> {
    let after = || (index..rows.len()).find(|&candidate| !is_role_group(&rows[candidate]));
    let before = || {
        (0..=index.min(rows.len().saturating_sub(1)))
            .rev()
            .find(|&candidate| !is_role_group(&rows[candidate]))
    };
    if forward {
        after().or_else(before)
    } else {
        before().or_else(after)
    }
}

/// One logical session across the fleet's machines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClientSessionRow {
    pub(super) name: String,
    /// Per configured machine, in configuration order: whether the session runs there, or
    /// `None` when the machine did not report it.
    pub(super) machines: Vec<Option<bool>>,
    pub(super) current: bool,
}

pub(super) fn session_rows(fleet: &crate::client::fleet::State) -> Vec<ClientSessionRow> {
    let current = fleet.config.origin_session.as_str();
    let machine_count = fleet.machines.len();
    let mut sessions = std::collections::BTreeMap::<&str, Vec<Option<bool>>>::new();
    for (index, machine) in fleet.machines.iter().enumerate() {
        for session in &machine.listed {
            sessions
                .entry(session.name.as_str())
                .or_insert_with(|| vec![None; machine_count])[index] = Some(session.running);
        }
        // This window is attached to the local origin session, so it runs even before the
        // first discovery result arrives.
        if machine.machine.id == fleet.config.local_machine {
            sessions
                .entry(current)
                .or_insert_with(|| vec![None; machine_count])[index] = Some(true);
        }
    }
    sessions
        .into_iter()
        .map(|(name, machines)| ClientSessionRow {
            name: name.to_owned(),
            machines,
            current: name == current,
        })
        .collect()
}

pub(super) fn session_selected_index(
    rows: &[ClientSessionRow],
    picker: &ClientSessionPickerOverlay,
) -> Option<usize> {
    picker
        .selected
        .as_deref()
        .and_then(|selected| rows.iter().position(|row| row.name == selected))
        .or_else(|| rows.iter().position(|row| row.current))
        .or_else(|| (!rows.is_empty()).then_some(0))
}

impl ClientShellState {
    /// Opens the floating session picker for combined windows.
    pub(super) fn open_session_picker(&mut self) {
        let Some(fleet) = self.fleet.as_ref() else {
            return;
        };
        self.overlay = Some(ClientShellOverlay::SessionPicker(
            ClientSessionPickerOverlay {
                selected: Some(fleet.config.origin_session.clone()),
                scroll: 0,
            },
        ));
    }

    /// Opens the floating agent picker grouped by the agents' `$role` metadata.
    pub(super) fn open_role_picker(&mut self) {
        let mut navigator = ClientNavigatorOverlay {
            query: TextEditor::default(),
            search_focused: true,
            selected: None,
            scroll: 0,
            filter: None,
            mode: ClientNavigatorMode::Roles {
                order: self.config.roles.clone(),
            },
        };
        let rows =
            render::client_navigator_rows(&self.endpoints, &self.active_endpoint_id, &navigator);
        navigator.selected = rows
            .iter()
            .find(|row| row.current)
            .map(|row| row.target.clone());
        self.overlay = Some(ClientShellOverlay::Navigator(navigator));
    }

    pub(super) fn jump_role_group(&mut self, jump: RoleGroupJump) {
        let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() else {
            return;
        };
        let rows =
            render::client_navigator_rows(&self.endpoints, &self.active_endpoint_id, navigator);
        let headings = rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| is_role_group(row).then_some(index))
            .collect::<Vec<_>>();
        if headings.is_empty() {
            return;
        }
        let selected = super::aggregate_navigation::navigator_selected_index(&rows, navigator)
            .unwrap_or_default();
        let current = headings
            .iter()
            .rposition(|&heading| heading <= selected)
            .unwrap_or_default();
        let destination = match jump {
            RoleGroupJump::Next => (current + 1) % headings.len(),
            RoleGroupJump::Previous => (current + headings.len() - 1) % headings.len(),
            RoleGroupJump::Index(index) if index < headings.len() => index,
            RoleGroupJump::Index(_) => return,
        };
        if let Some(row) = rows.get(headings[destination] + 1) {
            navigator.selected = Some(row.target.clone());
        }
    }

    /// Keys for the by-role navigator mode. Typing always edits the filter, so digits and
    /// Tab are taken for group jumps instead of the tree mode's letter shortcuts.
    pub(super) fn route_role_picker_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        outcome.repaint = true;
        match code {
            KeyCode::Esc => self.overlay = None,
            KeyCode::Enter => self.accept_navigator_selection(outcome),
            KeyCode::Tab if modifiers.is_empty() => self.jump_role_group(RoleGroupJump::Next),
            KeyCode::BackTab => self.jump_role_group(RoleGroupJump::Previous),
            KeyCode::Char(digit @ '1'..='9') if modifiers.is_empty() => {
                let index = digit as usize - '1' as usize;
                self.jump_role_group(RoleGroupJump::Index(index));
            }
            KeyCode::Up if modifiers.is_empty() => self.move_navigator_selection(-1),
            KeyCode::Down if modifiers.is_empty() => self.move_navigator_selection(1),
            KeyCode::Char('p') if modifiers == KeyModifiers::CONTROL => {
                self.move_navigator_selection(-1)
            }
            KeyCode::Char('n') if modifiers == KeyModifiers::CONTROL => {
                self.move_navigator_selection(1)
            }
            KeyCode::PageUp => self.move_navigator_selection(-8),
            KeyCode::PageDown => self.move_navigator_selection(8),
            _ => {
                if let Some(ClientShellOverlay::Navigator(navigator)) = self.overlay.as_mut() {
                    if navigator.query.handle_key(key) == Some(true) {
                        navigator.selected = None;
                        navigator.scroll = 0;
                    }
                }
            }
        }
    }

    fn move_session_selection(&mut self, delta: isize) {
        let Some(fleet) = self.fleet.as_ref() else {
            return;
        };
        let Some(ClientShellOverlay::SessionPicker(picker)) = self.overlay.as_mut() else {
            return;
        };
        let rows = session_rows(fleet);
        let Some(selected) = session_selected_index(&rows, picker) else {
            return;
        };
        let next =
            (selected as isize + delta).clamp(0, rows.len().saturating_sub(1) as isize) as usize;
        picker.selected = Some(rows[next].name.clone());
    }

    /// The current session closes the picker. Any other session needs its own window: the
    /// local endpoint is this process's attachment to the origin session, and the fleet
    /// configuration pins `origin_session` to it, so the client exits and relaunches scoped to
    /// the chosen session instead of re-scoping in place.
    pub(super) fn accept_session_selection(&mut self, outcome: &mut ClientShellInput) {
        outcome.repaint = true;
        let Some(fleet) = self.fleet.as_ref() else {
            self.overlay = None;
            return;
        };
        let Some(ClientShellOverlay::SessionPicker(picker)) = self.overlay.as_ref() else {
            return;
        };
        let rows = session_rows(fleet);
        let Some(row) = session_selected_index(&rows, picker).map(|index| &rows[index]) else {
            return;
        };
        let launch = (!row.current)
            .then(|| crate::client::fleet::launch::open(fleet, &row.name))
            .transpose();
        self.overlay = None;
        match launch {
            Ok(launch) => outcome.launch = launch,
            Err(notice) => self.set_endpoint_error(notice),
        }
    }

    /// Prompts for a new session name, targeting the machine this window has selected.
    pub(super) fn open_new_session_prompt(&mut self) {
        let Some(fleet) = self.fleet.as_ref() else {
            return;
        };
        self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "new session",
            input: TextEditor::default(),
            target: ClientRenameTarget::NewSession {
                machine: fleet.selected.machine.clone(),
            },
        }));
    }

    /// Moves the new-session prompt to the next or previous configured machine.
    pub(super) fn cycle_new_session_machine(&mut self, forward: bool) {
        let Some(fleet) = self.fleet.as_ref() else {
            return;
        };
        let Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            target: ClientRenameTarget::NewSession { machine },
            ..
        })) = self.overlay.as_mut()
        else {
            return;
        };
        let machines = &fleet.config.machines;
        let current = machines
            .iter()
            .position(|candidate| &candidate.id == machine)
            .unwrap_or_default();
        let next = if forward {
            (current + 1) % machines.len()
        } else {
            (current + machines.len() - 1) % machines.len()
        };
        *machine = machines[next].id.clone();
    }

    /// Leaves the window to create the session, or keeps the prompt open while the name is
    /// invalid; the prompt renders the reason.
    pub(super) fn save_new_session_prompt(&mut self, outcome: &mut ClientShellInput) {
        outcome.repaint = true;
        let Some(fleet) = self.fleet.as_ref() else {
            self.overlay = None;
            return;
        };
        let Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            input,
            target: ClientRenameTarget::NewSession { machine },
            ..
        })) = self.overlay.as_ref()
        else {
            return;
        };
        if let Ok(launch) = crate::client::fleet::launch::create(fleet, machine, input.trim()) {
            self.overlay = None;
            outcome.launch = Some(launch);
        }
    }

    pub(super) fn route_session_picker_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        outcome.repaint = true;
        match code {
            KeyCode::Esc => self.overlay = None,
            KeyCode::Enter => self.accept_session_selection(outcome),
            KeyCode::Char('n') if modifiers.is_empty() => self.open_new_session_prompt(),
            KeyCode::Up | KeyCode::Char('k') if modifiers.is_empty() => {
                self.move_session_selection(-1)
            }
            KeyCode::Down | KeyCode::Char('j') if modifiers.is_empty() => {
                self.move_session_selection(1)
            }
            KeyCode::Home if modifiers.is_empty() => self.move_session_selection(isize::MIN / 2),
            KeyCode::End if modifiers.is_empty() => self.move_session_selection(isize::MAX / 2),
            _ => {}
        }
    }

    pub(super) fn handle_session_picker_mouse(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) {
        let point = (mouse.column, mouse.row);
        let row_hit = self
            .hits
            .session_rows
            .iter()
            .find(|(rect, _)| super::contains(*rect, point))
            .map(|(_, name)| name.clone());
        match mouse.kind {
            MouseEventKind::Moved => {
                if let (Some(name), Some(ClientShellOverlay::SessionPicker(picker))) =
                    (row_hit, self.overlay.as_mut())
                {
                    picker.selected = Some(name);
                    outcome.repaint = true;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(name) = row_hit {
                    if let Some(ClientShellOverlay::SessionPicker(picker)) = self.overlay.as_mut() {
                        picker.selected = Some(name);
                    }
                    self.accept_session_selection(outcome);
                } else if !super::contains(self.hits.session_popup, point) {
                    self.overlay = None;
                    outcome.repaint = true;
                }
            }
            MouseEventKind::ScrollUp => {
                self.move_session_selection(-3);
                outcome.repaint = true;
            }
            MouseEventKind::ScrollDown => {
                self.move_session_selection(3);
                outcome.repaint = true;
            }
            _ => {}
        }
    }
}
