use super::render::{display_width, put_text, ShellRenderState};
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

/// `▾ khion  local          ●`: disclosure, machine, transport, then the endpoint signal.
pub(super) fn render_machine(
    buffer: &mut Buffer,
    rect: Rect,
    machine: usize,
    endpoint: Option<usize>,
    state: &ShellRenderState<'_>,
    fleet: &crate::client::fleet::State,
    palette: &Palette,
    materials: &crate::ui::Materials,
) -> FleetMachineHit {
    let group = &fleet.machines[machine];
    let collapsed = fleet.collapsed.contains(&group.machine.id);
    let endpoint = endpoint.map(|index| &state.endpoints[index]);
    let (signal, signal_style) = match endpoint {
        Some(endpoint) => super::endpoint_sidebar::endpoint_signal(
            endpoint,
            state.machine_diagnostics,
            palette,
            true,
        ),
        None => (
            if group.error.is_some() {
                "offline"
            } else if !group.discovered {
                "discovering"
            } else {
                "not running"
            }
            .to_owned(),
            Style::default()
                .fg(palette.overlay0)
                .add_modifier(Modifier::DIM),
        ),
    };
    let signal_width = display_width(&signal).min(rect.width.saturating_sub(1));
    let signal_rect = Rect::new(
        rect.right().saturating_sub(signal_width.saturating_add(1)),
        rect.y,
        signal_width,
        1,
    );
    let text_end = signal_rect.x.saturating_sub(1);
    let disabled =
        endpoint.is_none_or(|endpoint| endpoint.status == ClientEndpointStatus::Disabled);
    let mut x = super::render::put_segment(
        buffer,
        rect.x.saturating_add(1),
        rect.y,
        text_end,
        if collapsed { "▸ " } else { "▾ " },
        Style::default().fg(palette.overlay0),
    );
    x = super::render::put_segment(
        buffer,
        x,
        rect.y,
        text_end,
        &group.machine.label,
        Style::default()
            .fg(if disabled {
                palette.overlay0
            } else {
                palette.text
            })
            .add_modifier(Modifier::BOLD),
    );
    if x.saturating_add(2) < text_end {
        super::render::put_segment(
            buffer,
            x.saturating_add(2),
            rect.y,
            text_end,
            if group.machine.target.is_none() {
                "local"
            } else {
                "ssh"
            },
            Style::default().fg(palette.overlay0),
        );
    }
    put_text(
        buffer,
        signal_rect.x,
        rect.y,
        signal_rect.width,
        &signal,
        signal_style,
    );
    // Expanded machines leave the focus highlight to their active space.
    if collapsed
        && endpoint.is_some_and(|endpoint| &endpoint.endpoint_id == state.active_endpoint_id)
    {
        super::sidebar_frame::paint_row(
            buffer,
            rect,
            super::sidebar_frame::RowTone::Focused,
            materials,
        );
    }
    FleetMachineHit {
        rect,
        collapse_toggle: Rect::new(
            rect.x.saturating_add(1),
            rect.y,
            u16::from(rect.width > 1),
            u16::from(rect.height > 0),
        ),
        status_badge: if endpoint.is_some() {
            signal_rect
        } else {
            Rect::default()
        },
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
