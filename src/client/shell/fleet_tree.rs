use super::render::{display_width, put_right_text, put_text, ShellRenderState};
use super::*;

pub(super) struct FleetMachineHit {
    pub(super) rect: Rect,
    pub(super) collapse_toggle: Rect,
    pub(super) status_badge: Rect,
    pub(super) machine_id: String,
    pub(super) endpoint_id: Option<ClientEndpointId>,
}

pub(super) struct WorkspaceRegion {
    pub(super) rect: Rect,
    pub(super) endpoint_id: ClientEndpointId,
}

#[derive(Clone, Copy)]
pub(super) struct Branch {
    pub(super) last_root: bool,
}

pub(super) enum Row {
    Machine {
        machine: usize,
        endpoint: Option<usize>,
    },
    Workspace {
        endpoint: usize,
        entry: WorkspaceEntry,
        branch: Branch,
    },
}

pub(super) fn rows(
    state: &ShellRenderState<'_>,
    fleet: &crate::client::fleet::State,
    mut emit: impl FnMut(Row),
) {
    let empty = HashSet::new();
    for (machine, group) in fleet.machines.iter().enumerate() {
        let endpoint = state.endpoints.iter().position(|endpoint| {
            fleet.machine_for(&endpoint.endpoint_id) == Some(group.machine.id.as_str())
        });
        emit(Row::Machine { machine, endpoint });
        if fleet.collapsed.contains(&group.machine.id) {
            continue;
        }
        let Some(index) = endpoint else {
            continue;
        };
        let endpoint = &state.endpoints[index];
        let Some(snapshot) = endpoint.snapshot.as_deref() else {
            continue;
        };
        let collapsed = if endpoint.endpoint_id.is_local() {
            state.collapsed_groups
        } else {
            state
                .remote_collapsed_groups
                .get(&endpoint.endpoint_id)
                .unwrap_or(&empty)
        };
        let entries = super::sidebar::workspace_entries(snapshot, collapsed);
        let last_root = entries.iter().rposition(|entry| !entry.indented);
        let mut branch = Branch { last_root: false };
        for (position, entry) in entries.into_iter().enumerate() {
            if !entry.indented {
                branch.last_root = Some(position) == last_root;
            }
            emit(Row::Workspace {
                endpoint: index,
                entry,
                branch,
            });
        }
    }
}

pub(super) fn render_machine(
    buffer: &mut Buffer,
    rect: Rect,
    machine: usize,
    endpoint: Option<usize>,
    state: &ShellRenderState<'_>,
    fleet: &crate::client::fleet::State,
    palette: &Palette,
) -> FleetMachineHit {
    let group = &fleet.machines[machine];
    let collapsed = fleet.collapsed.contains(&group.machine.id);
    let marker = if collapsed { "▸" } else { "▾" };
    let label = if group.machine.id == fleet.config.local_machine {
        format!("{} · This device", group.machine.label)
    } else {
        group.machine.label.clone()
    };
    let endpoint = endpoint.map(|index| &state.endpoints[index]);
    let status_badge = if let Some(endpoint) = endpoint {
        super::endpoint_sidebar::render_endpoint_row(
            buffer,
            rect,
            marker,
            &label,
            endpoint,
            &endpoint.endpoint_id == state.active_endpoint_id,
            state.machine_diagnostics,
            palette,
        )
    } else {
        let signal = if group.error.is_some() {
            "offline"
        } else if !group.discovered {
            "discovering"
        } else {
            "not running"
        };
        let signal_width = display_width(signal).min(rect.width);
        put_text(
            buffer,
            rect.x,
            rect.y,
            rect.width.saturating_sub(signal_width.saturating_add(1)),
            &format!(" {marker} {label}"),
            Style::default()
                .fg(palette.text)
                .add_modifier(Modifier::BOLD),
        );
        put_right_text(
            buffer,
            rect,
            rect.y,
            signal,
            Style::default().fg(palette.overlay0),
        );
        Rect::default()
    };
    FleetMachineHit {
        rect,
        collapse_toggle: Rect::new(
            rect.x.saturating_add(1),
            rect.y,
            u16::from(rect.width > 1),
            u16::from(rect.height > 0),
        ),
        status_badge,
        machine_id: group.machine.id.clone(),
        endpoint_id: endpoint.map(|endpoint| endpoint.endpoint_id.clone()),
    }
}

pub(super) fn render_branch(
    buffer: &mut Buffer,
    rect: Rect,
    entry: &WorkspaceEntry,
    branch: Branch,
    gap: u16,
    body_bottom: u16,
    palette: &Palette,
) -> Rect {
    let end = rect.bottom().saturating_add(gap).min(body_bottom);
    for y in rect.y..end {
        let prefix = if !entry.indented && y == rect.y {
            if branch.last_root {
                "   └─"
            } else {
                "   ├─"
            }
        } else if branch.last_root {
            "     "
        } else {
            "   │ "
        };
        put_text(
            buffer,
            rect.x,
            y,
            rect.width.min(5),
            prefix,
            Style::default().fg(palette.overlay0),
        );
    }
    let indent = rect.width.min(5);
    Rect::new(
        rect.x.saturating_add(indent),
        rect.y,
        rect.width.saturating_sub(indent),
        rect.height,
    )
}
