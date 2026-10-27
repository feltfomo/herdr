//! Machine-grouped agent panel for combined windows.

use std::borrow::Cow;

use super::render::{display_width, put_text};
use super::sidebar_frame::{label_style, paint_row, RowTone, SidebarFrame};
use super::*;
use crate::protocol::ClientShellAgent;
use crate::ui::Materials;

pub(super) enum Item<'a> {
    Machine { machine: usize, collapsed: bool },
    Agent(Entry<'a>),
}

pub(super) struct Entry<'a> {
    endpoint_id: &'a ClientEndpointId,
    agent: &'a ClientShellAgent,
    workspace_label: &'a str,
    focused: bool,
    stale: bool,
    last: bool,
    path: Option<Cow<'a, str>>,
    meta: Option<String>,
}

impl Item<'_> {
    fn height(&self) -> u16 {
        match self {
            Self::Machine { .. } => 1,
            Self::Agent(entry) => 1 + u16::from(entry.path.is_some() || entry.meta.is_some()),
        }
    }
}

/// Agents visible under the fleet scope, grouped under their machine in fleet order and
/// ordered by urgency within each machine. Ties keep the aggregate order, so the agent
/// view and recency still break them.
pub(super) fn items<'a>(
    endpoints: &'a [ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    fleet: &crate::client::fleet::State,
    collapsed: &HashSet<String>,
    config: &ClientShellConfig,
) -> Vec<Item<'a>> {
    let mut grouped = (0..fleet.machines.len())
        .map(|_| Vec::new())
        .collect::<Vec<_>>();
    for row in super::aggregate_navigation::aggregate_agent_rows(
        endpoints,
        active_endpoint_id,
        config.agent_panel_sort,
    ) {
        let machine = fleet.machine_for(row.endpoint.endpoint_id).and_then(|id| {
            fleet
                .machines
                .iter()
                .position(|group| group.machine.id == id)
        });
        if let Some(machine) = machine {
            grouped[machine].push(row);
        }
    }
    let detailed = config.agent_detail == crate::config::AgentDetailConfig::Detailed;
    let mut items = Vec::new();
    for (machine, mut rows) in grouped.into_iter().enumerate() {
        if rows.is_empty() {
            continue;
        }
        let machine_collapsed = collapsed.contains(&fleet.machines[machine].machine.id);
        items.push(Item::Machine {
            machine,
            collapsed: machine_collapsed,
        });
        if machine_collapsed {
            continue;
        }
        rows.sort_by_key(|row| std::cmp::Reverse(status_priority(row.agent.agent_status)));
        let count = rows.len();
        for (index, row) in rows.into_iter().enumerate() {
            let snapshot = row.endpoint.snapshot;
            let workspace = snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == row.agent.workspace_id);
            let (path, meta) = if detailed {
                (
                    agent_path(snapshot, workspace, row.agent, config),
                    agent_meta(row.agent),
                )
            } else {
                (None, None)
            };
            items.push(Item::Agent(Entry {
                endpoint_id: row.endpoint.endpoint_id,
                agent: row.agent,
                workspace_label: workspace.map_or("", |workspace| workspace.label.as_str()),
                focused: row.agent.focused && row.endpoint.endpoint_id == active_endpoint_id,
                stale: row.endpoint.stale(),
                last: index + 1 == count,
                path,
                meta,
            }));
        }
    }
    items
}

fn layout(items: &[Item<'_>], row_gap: u16) -> (Vec<u16>, Vec<u16>) {
    let heights = items.iter().map(Item::height).collect();
    let gaps = items
        .iter()
        .zip(items.iter().skip(1).map(Some).chain(std::iter::once(None)))
        .map(|pair| match pair {
            (Item::Agent(_), Some(Item::Agent(_))) => row_gap,
            _ => 0,
        })
        .collect();
    (heights, gaps)
}

/// The pane's live directory, shown only when it says more than the space root does.
fn agent_path<'a>(
    snapshot: &'a ClientShellSnapshot,
    workspace: Option<&'a ClientShellWorkspace>,
    agent: &ClientShellAgent,
    config: &ClientShellConfig,
) -> Option<Cow<'a, str>> {
    if !config.show_agent_path {
        return None;
    }
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == agent.pane_id)?;
    let path = pane
        .foreground_cwd
        .as_deref()
        .or(pane.cwd.as_deref())
        .filter(|path| !path.is_empty())?;
    let root = workspace.map(|workspace| workspace.new_workspace_cwd.as_str());
    if root.is_some_and(|root| trim_separator(root) == trim_separator(path)) {
        return None;
    }
    Some(home_relative(path))
}

fn trim_separator(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        path
    } else {
        trimmed
    }
}

fn agent_meta(agent: &ClientShellAgent) -> Option<String> {
    let token = |name: &str| {
        agent
            .tokens
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
    };
    match (token("model"), token("role")) {
        (Some(model), Some(role)) => Some(format!("{model} · {role}")),
        (Some(value), None) | (None, Some(value)) => Some(value.to_owned()),
        (None, None) => None,
    }
}

/// Display-only `~` for home directories. Remote paths belong to another machine, so
/// this matches the conventional home roots textually instead of asking the local OS.
pub(super) fn home_relative(path: &str) -> Cow<'_, str> {
    for root in ["/home/", "/Users/"] {
        if let Some(rest) = path.strip_prefix(root) {
            let (user, tail) = rest.split_once('/').unwrap_or((rest, ""));
            if !user.is_empty() {
                return home_tail(tail);
            }
        }
    }
    if path == "/root" {
        return Cow::Borrowed("~");
    }
    if let Some(tail) = path.strip_prefix("/root/") {
        return home_tail(tail);
    }
    Cow::Borrowed(path)
}

fn home_tail(tail: &str) -> Cow<'_, str> {
    if tail.is_empty() {
        Cow::Borrowed("~")
    } else {
        Cow::Owned(format!("~/{tail}"))
    }
}

/// Shortens leading directories to their first character, fish-prompt style, only when
/// the full path does not fit.
pub(super) fn fit_path(path: &str, width: usize) -> Cow<'_, str> {
    if unicode_width::UnicodeWidthStr::width(path) <= width {
        return Cow::Borrowed(path);
    }
    let Some((parents, last)) = path.rsplit_once('/') else {
        return Cow::Borrowed(path);
    };
    let mut short = String::with_capacity(path.len());
    for part in parents.split('/') {
        if let Some(first) = part.chars().next() {
            short.push(first);
        }
        short.push('/');
    }
    short.push_str(last);
    Cow::Owned(short)
}

pub(super) fn render(
    buffer: &mut Buffer,
    frame: &SidebarFrame,
    materials: &Materials,
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    fleet: &crate::client::fleet::State,
    collapsed: &HashSet<String>,
    agent_view_label: Option<&str>,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    hits.fleet_scope = frame.render_agent_header(
        buffer,
        (
            fleet.scope.label(),
            Style::default()
                .fg(palette.overlay0)
                .add_modifier(Modifier::BOLD),
        ),
        materials,
        palette,
    );
    hits.sidebar_section_divider = frame.section_divider_excluding(hits.fleet_scope);
    let items = items(endpoints, active_endpoint_id, fleet, collapsed, config);
    let (_, gaps) = layout(&items, config.agents.row_gap);
    super::agent_sidebar::render_agent_list(
        buffer,
        frame.agent_body,
        &items,
        &gaps,
        agent_view_label.map(|_| " no matching agents"),
        config,
        agent_scroll,
        hits,
        |item| usize::from(item.height()),
        |buffer, rect, gap, item, hits| match item {
            Item::Machine { machine, collapsed } => {
                let group = &fleet.machines[*machine];
                let row = Rect::new(rect.x, rect.y, rect.width, 1);
                put_text(
                    buffer,
                    row.x.saturating_add(1),
                    row.y,
                    row.width.saturating_sub(1),
                    if *collapsed { "▸" } else { "▾" },
                    Style::default().fg(palette.overlay0),
                );
                put_text(
                    buffer,
                    row.x.saturating_add(3),
                    row.y,
                    row.width.saturating_sub(3),
                    &group.machine.label,
                    label_style(palette),
                );
                hits.fleet_agent_machines
                    .push((row, group.machine.id.clone()));
            }
            Item::Agent(entry) => {
                render_entry(buffer, rect, gap, entry, config, materials);
                hits.endpoint_agents.push((
                    rect,
                    entry.endpoint_id.clone(),
                    entry.agent.pane_id.clone(),
                ));
            }
        },
    );
}

fn render_entry(
    buffer: &mut Buffer,
    rect: Rect,
    gap: u16,
    entry: &Entry<'_>,
    config: &ClientShellConfig,
    materials: &Materials,
) {
    let palette = &config.palette;
    let agent = entry.agent;
    let connector = Style::default().fg(palette.overlay0);
    let (branch, rail) = if entry.last {
        ("   └─", "     ")
    } else {
        ("   ├─", "   │ ")
    };
    put_text(buffer, rect.x, rect.y, rect.width.min(5), branch, connector);
    for y in rect.y.saturating_add(1)..rect.bottom().saturating_add(gap) {
        put_text(buffer, rect.x, y, rect.width.min(5), rail, connector);
    }

    let status = Style::default().fg(status_color(agent.agent_status, palette));
    let right = rect.right().saturating_sub(1);
    let mut x = rect.x.saturating_add(5);
    x = super::render::put_segment(
        buffer,
        x,
        rect.y,
        right,
        status_icon(agent.agent_status, config.status_indicators),
        status,
    );
    x = x.saturating_add(1);
    let state = agent
        .state_labels
        .iter()
        .find(|(state, _)| state == status_text(agent.agent_status))
        .map_or_else(
            || super::agent_sidebar::sidebar_status_text(agent.agent_status),
            |(_, label)| label.as_str(),
        );
    let state_width = display_width(state);
    let state_x = right.saturating_sub(state_width);
    if state_x > x {
        put_text(buffer, state_x, rect.y, state_width, state, status);
    }
    let label_end = state_x.saturating_sub(1).max(x);
    let label = agent
        .display_agent
        .as_deref()
        .or(agent.name.as_deref())
        .or(agent.agent.as_deref())
        .or(agent.title.as_deref())
        .unwrap_or("agent");
    x = super::render::put_segment(
        buffer,
        x,
        rect.y,
        label_end,
        label,
        Style::default()
            .fg(if entry.focused {
                palette.text
            } else {
                palette.subtext0
            })
            .add_modifier(Modifier::BOLD),
    );
    if !entry.workspace_label.is_empty() && x.saturating_add(1) < label_end {
        super::render::put_segment(
            buffer,
            x.saturating_add(1),
            rect.y,
            label_end,
            entry.workspace_label,
            Style::default().fg(palette.overlay0),
        );
    }

    if rect.height > 1 {
        let detail_x = rect.x.saturating_add(7);
        let room = usize::from(right.saturating_sub(detail_x));
        let meta = entry.meta.as_deref();
        let meta_room = meta.map_or(0, |meta| {
            unicode_width::UnicodeWidthStr::width(meta).saturating_add(3)
        });
        let detail = match (entry.path.as_deref(), meta) {
            (Some(path), Some(meta)) => {
                format!(
                    "{} · {meta}",
                    fit_path(path, room.saturating_sub(meta_room))
                )
            }
            (Some(path), None) => fit_path(path, room).into_owned(),
            (None, Some(meta)) => meta.to_owned(),
            (None, None) => String::new(),
        };
        put_text(
            buffer,
            detail_x,
            rect.y.saturating_add(1),
            right.saturating_sub(detail_x),
            &detail,
            Style::default().fg(palette.overlay0),
        );
    }

    if entry.stale {
        buffer.set_style(
            rect,
            Style::default()
                .fg(palette.overlay0)
                .add_modifier(Modifier::DIM),
        );
    }
    if entry.focused {
        paint_row(buffer, rect, RowTone::Focused, materials);
    }
}

impl ClientShellState {
    pub(super) fn reveal_fleet_agent(
        &mut self,
        endpoint_id: &ClientEndpointId,
        pane_id: &str,
        body_height: u16,
    ) {
        let Some(fleet) = self.fleet.as_ref() else {
            return;
        };
        if let Some(machine) = fleet.machine_for(endpoint_id) {
            self.fleet_agent_collapsed.remove(machine);
        }
        let items = items(
            &self.endpoints,
            &self.active_endpoint_id,
            fleet,
            &self.fleet_agent_collapsed,
            &self.config,
        );
        let Some(target) = items.iter().position(|item| {
            matches!(item, Item::Agent(entry)
                if entry.endpoint_id == endpoint_id && entry.agent.pane_id == pane_id)
        }) else {
            return;
        };
        let (heights, gaps) = layout(&items, self.config.agents.row_gap);
        let scroll = super::scroll::list_scroll_start_to_reveal(
            &heights,
            &gaps,
            body_height,
            self.agent_scroll,
            target,
        );
        self.agent_scroll = scroll;
    }

    pub(super) fn toggle_fleet_agent_machine(&mut self, machine: &str) {
        if !self.fleet_agent_collapsed.remove(machine) {
            self.fleet_agent_collapsed.insert(machine.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_roots_abbreviate_textually_without_touching_other_paths() {
        assert_eq!(
            home_relative("/home/can/Projects/herdr"),
            "~/Projects/herdr"
        );
        assert_eq!(home_relative("/Users/can"), "~");
        assert_eq!(home_relative("/root/src"), "~/src");
        assert_eq!(home_relative("/homework/x"), "/homework/x");
        assert_eq!(home_relative("/srv/build"), "/srv/build");
    }

    #[test]
    fn paths_shorten_parents_only_when_they_overflow() {
        assert_eq!(fit_path("~/Projects/skadi", 20), "~/Projects/skadi");
        assert_eq!(fit_path("~/Projects/skadi", 10), "~/P/skadi");
        assert_eq!(fit_path("/srv/very/long/tree", 8), "/s/v/l/tree");
    }
}
