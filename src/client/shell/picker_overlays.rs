use super::*;
use crate::ui::Materials;

const ROLE_FOOTER: &str = " enter focus · tab next role · 1-9 jump · esc";
const SESSION_FOOTER: &str = " enter open · n new · esc";

/// A raised panel with its label set into the top border and an optional right-hand label.
fn floating_panel(
    b: &mut Buffer,
    area: Rect,
    title: &str,
    right: Option<&str>,
    m: &Materials,
    p: &Palette,
) -> Option<Rect> {
    let inner = panel(b, area, m.strong_border, m.raised)?;
    let title = format!(" {title} ");
    let title_width = display_width(&title).min(area.width.saturating_sub(4));
    put_text(
        b,
        area.x + 2,
        area.y,
        title_width,
        &title,
        Style::default()
            .fg(p.text)
            .bg(m.raised)
            .add_modifier(Modifier::BOLD),
    );
    if let Some(right) = right {
        let right = format!(" {right} ");
        let width = display_width(&right);
        let x = area.right().saturating_sub(width + 2);
        if x > area.x + 2 + title_width {
            put_text(
                b,
                x,
                area.y,
                width,
                &right,
                Style::default().fg(p.subtext0).bg(m.raised),
            );
        }
    }
    Some(inner)
}

fn divider(b: &mut Buffer, area: Rect, y: u16, m: &Materials) {
    let style = Style::default().fg(m.strong_border).bg(m.raised);
    for x in area.x..area.right() {
        let symbol = if x == area.x {
            "├"
        } else if x + 1 == area.right() {
            "┤"
        } else {
            "─"
        };
        b[(x, y)].set_symbol(symbol).set_style(style);
    }
}

fn centered(a: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(a.width.saturating_sub(4));
    let height = height.min(a.height.saturating_sub(2));
    Rect::new(
        a.x + (a.width - width) / 2,
        a.y + (a.height - height) / 2,
        width,
        height,
    )
}

fn scroll_window(scroll: usize, selected: usize, len: usize, viewport: u16) -> (usize, usize) {
    let max = len.saturating_sub(viewport as usize);
    let scroll = scroll
        .max(selected.saturating_sub(viewport.saturating_sub(1) as usize))
        .min(selected)
        .min(max);
    (scroll, max)
}

pub(super) fn render_role_picker(
    b: &mut Buffer,
    n: &ClientNavigatorOverlay,
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    fleet: Option<&crate::client::fleet::State>,
    p: &Palette,
) -> Option<OverlayRender> {
    let m = Materials::from_palette(p);
    let rows = super::super::aggregate_navigation::navigator_rows(endpoints, active_endpoint_id, n);
    let height = (rows.len() as u16).saturating_add(5).clamp(9, 32);
    let q = centered(b.area, 76, height);
    if q.width < 24 || q.height < 7 {
        return None;
    }
    let session = fleet.map(|fleet| fleet.config.origin_session.as_str());
    let i = floating_panel(b, q, "agents by role", session, &m, p)?;

    let field = Rect::new(i.x, i.y, i.width, 1);
    b.set_style(field, Style::default().bg(m.field));
    put_text(
        b,
        field.x,
        field.y,
        3,
        " ⌕ ",
        Style::default().fg(p.overlay0).bg(m.field),
    );
    let editor = Rect::new(field.x + 3, field.y, field.width.saturating_sub(4), 1);
    let cursor = text_editor::render(b, editor, &n.query, Style::default().fg(p.text).bg(m.field));
    if n.query.is_empty() {
        put_text(
            b,
            editor.x,
            editor.y,
            editor.width,
            "filter role, agent, space, machine, model",
            Style::default().fg(p.overlay0).bg(m.field),
        );
    }

    let footer_y = i.bottom() - 1;
    divider(b, q, footer_y - 1, &m);
    put_text(
        b,
        i.x,
        footer_y,
        i.width,
        ROLE_FOOTER,
        Style::default().fg(p.overlay0).bg(m.raised),
    );

    let body = Rect::new(i.x, i.y + 1, i.width, i.height.saturating_sub(3));
    let selected = super::super::aggregate_navigation::navigator_selected_index(&rows, n);
    let (scroll, max) = scroll_window(
        n.scroll,
        selected.unwrap_or_default(),
        rows.len(),
        body.height,
    );
    let metrics = crate::pane::ScrollMetrics {
        offset_from_bottom: max.saturating_sub(scroll),
        max_offset_from_bottom: max,
        viewport_rows: usize::from(body.height),
    };
    let scrollbar =
        (max > 0 && body.width > 1).then_some(Rect::new(body.right() - 1, body.y, 1, body.height));
    let row_width = body.width.saturating_sub(u16::from(scrollbar.is_some()));
    if rows.is_empty() {
        put_text(
            b,
            body.x,
            body.y,
            body.width,
            " no matching agents",
            Style::default().fg(p.overlay0).bg(m.raised),
        );
    }

    let column = |value: fn(&ClientNavigatorRow) -> &str, cap: usize| {
        rows.iter()
            .filter(|row| row.status.is_some())
            .map(|row| display_width(value(row)) as usize)
            .max()
            .unwrap_or_default()
            .min(cap)
    };
    let agent_width = column(|row| &row.label, 10);
    let space_width = column(|row| &row.detail, 14);
    let machine_width = column(|row| &row.machine, 10);
    let model_width = column(|row| &row.meta, 12);

    let mut heading = 0;
    let mut row_hits = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let Some(status) = row.status else {
            heading += 1;
            if index < scroll || index >= scroll + body.height as usize {
                continue;
            }
            let y = body.y + (index - scroll) as u16;
            let label = format!(" {}", row.label);
            let style = Style::default()
                .fg(p.accent)
                .bg(m.raised)
                .add_modifier(Modifier::BOLD);
            put_text(b, body.x, y, row_width, &label, style);
            if heading <= 9 {
                let x = body.x + display_width(&label) + 2;
                put_text(
                    b,
                    x,
                    y,
                    body.right().saturating_sub(x),
                    &heading.to_string(),
                    Style::default().fg(p.overlay0).bg(m.raised),
                );
            }
            continue;
        };
        if index < scroll || index >= scroll + body.height as usize {
            continue;
        }
        let rect = Rect::new(body.x, body.y + (index - scroll) as u16, row_width, 1);
        row_hits.push((rect, row.target.clone()));
        let is_selected = selected == Some(index);
        let base = if is_selected {
            Style::default()
                .fg(m.selected_fg)
                .bg(m.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.text).bg(m.raised)
        };
        let base = if row.stale {
            base.add_modifier(Modifier::DIM)
        } else {
            base
        };
        let muted = if is_selected {
            base
        } else {
            base.fg(p.subtext0)
        };
        b.set_style(rect, base);
        let glyph = status_dot(status);
        put_text(
            b,
            rect.x + 3,
            rect.y,
            2,
            glyph,
            if is_selected {
                base
            } else {
                base.fg(status_color(status, p))
            },
        );
        let state = format!("{} ", status_text(status));
        let state_width = display_width(&state);
        let right = rect.right().saturating_sub(state_width);
        let mut x = rect.x + 5;
        for (value, width, style) in [
            (row.label.as_str(), agent_width, base),
            (row.detail.as_str(), space_width, muted),
            (row.machine.as_str(), machine_width, muted),
            (row.meta.as_str(), model_width, muted),
        ] {
            if width == 0 {
                continue;
            }
            let width = (width as u16).min(right.saturating_sub(x));
            put_text(b, x, rect.y, width, value, style);
            x = x.saturating_add(width).saturating_add(2);
        }
        put_right_text(b, rect, rect.y, &state, muted);
    }
    if let Some(track) = scrollbar {
        crate::ui::render_scrollbar_buffer(b, metrics, track, p.overlay0, p.overlay1, "▐");
    }
    Some(OverlayRender {
        area: q,
        navigator_popup: q,
        navigator_search: field,
        navigator_rows: row_hits,
        navigator_scrollbar: scrollbar.unwrap_or_default(),
        navigator_scroll_metrics: Some(metrics),
        cursor,
        ..OverlayRender::default()
    })
}

pub(super) fn render_session_picker(
    b: &mut Buffer,
    picker: &ClientSessionPickerOverlay,
    fleet: &crate::client::fleet::State,
    p: &Palette,
) -> Option<OverlayRender> {
    let m = Materials::from_palette(p);
    let rows = super::super::pickers::session_rows(fleet);
    let name_width = rows
        .iter()
        .map(|row| display_width(&row.name))
        .chain([display_width("session")])
        .max()
        .unwrap_or_default()
        .min(24);
    let machine_widths = fleet
        .machines
        .iter()
        .map(|machine| display_width(&machine.machine.label).clamp(1, 12))
        .collect::<Vec<_>>();
    let width = machine_widths
        .iter()
        .map(|width| width + 2)
        .sum::<u16>()
        .saturating_add(name_width + 8)
        .max(display_width(SESSION_FOOTER) + 3)
        .max(28);
    let height = (rows.len() as u16).saturating_add(5).clamp(7, 24);
    let q = centered(b.area, width, height);
    if q.width < 16 || q.height < 6 {
        return None;
    }
    let current = fleet.config.origin_session.as_str();
    let i = floating_panel(b, q, "sessions", Some(current), &m, p)?;

    let columns_x = i.x + 3 + name_width + 2;
    let header = Style::default().fg(p.overlay0).bg(m.raised);
    put_text(b, i.x + 3, i.y, name_width, "session", header);
    let mut x = columns_x;
    for (machine, width) in fleet.machines.iter().zip(&machine_widths) {
        let style = if machine.error.is_some() {
            header.fg(p.red)
        } else {
            header
        };
        put_text(b, x, i.y, *width, &machine.machine.label, style);
        x = x.saturating_add(width + 2);
    }

    let footer_y = i.bottom() - 1;
    divider(b, q, footer_y - 1, &m);
    put_text(b, i.x, footer_y, i.width, SESSION_FOOTER, header);

    let body = Rect::new(i.x, i.y + 1, i.width, i.height.saturating_sub(3));
    let selected = super::super::pickers::session_selected_index(&rows, picker);
    let (scroll, _) = scroll_window(
        picker.scroll,
        selected.unwrap_or_default(),
        rows.len(),
        body.height,
    );
    let mut row_hits = Vec::new();
    for (index, row) in rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(body.height as usize)
    {
        let rect = Rect::new(body.x, body.y + (index - scroll) as u16, body.width, 1);
        row_hits.push((rect, row.name.clone()));
        let is_selected = selected == Some(index);
        let base = if is_selected {
            Style::default()
                .fg(m.selected_fg)
                .bg(m.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.text).bg(m.raised)
        };
        b.set_style(rect, base);
        if row.current {
            put_text(
                b,
                rect.x + 1,
                rect.y,
                1,
                "◆",
                if is_selected { base } else { base.fg(p.accent) },
            );
        }
        put_text(b, rect.x + 3, rect.y, name_width, &row.name, base);
        let mut x = columns_x;
        for (running, width) in row.machines.iter().zip(&machine_widths) {
            if let Some(running) = running {
                let (glyph, color) = if *running {
                    ("●", p.green)
                } else {
                    ("○", p.overlay0)
                };
                put_text(
                    b,
                    x + (width.saturating_sub(1)) / 2,
                    rect.y,
                    1,
                    glyph,
                    if is_selected { base } else { base.fg(color) },
                );
            }
            x = x.saturating_add(width + 2);
        }
    }
    Some(OverlayRender {
        area: q,
        session_popup: q,
        session_rows: row_hits,
        ..OverlayRender::default()
    })
}
