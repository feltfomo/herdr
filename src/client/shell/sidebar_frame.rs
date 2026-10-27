//! Geometry, surfaces and labelled borders shared by every expanded desktop sidebar.

use super::render::{display_width, put_right_text, put_text};
use super::*;
use crate::ui::Materials;

/// Bordered frames need room for the top border, a body row in each panel, the section
/// split, the footer split, the footer and the bottom border; smaller sidebars keep the
/// borderless layout instead of overlapping rules.
const MIN_BORDERED_HEIGHT: u16 = 10;
const MIN_BORDERED_WIDTH: u16 = 6;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SidebarFrame {
    pub(super) bordered: bool,
    area: Rect,
    /// Row carrying the workspace panel title; the top border when bordered.
    title: Rect,
    pub(super) workspace_body: Rect,
    /// Row carrying the agent panel label; the section split when bordered.
    agent_header: Rect,
    pub(super) agent_body: Rect,
    /// Inner row for the footer actions; empty when the footer is hidden.
    pub(super) footer: Rect,
    footer_split: Option<u16>,
    pub(super) section_divider: Rect,
    pub(super) toggle: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RowTone {
    Plain,
    Focused,
    Selected,
    Dragged,
}

pub(super) struct FooterActions {
    pub(super) sessions: bool,
    pub(super) attention: bool,
}

impl SidebarFrame {
    pub(super) fn layout(area: Rect, split_ratio: f32, bordered: bool, footer: bool) -> Self {
        if bordered && area.width >= MIN_BORDERED_WIDTH && area.height >= MIN_BORDERED_HEIGHT {
            Self::bordered(area, split_ratio, footer)
        } else {
            Self::borderless(area, split_ratio, footer)
        }
    }

    fn borderless(area: Rect, split_ratio: f32, footer: bool) -> Self {
        let (workspace, detail) = crate::ui::expanded_sidebar_sections(area, split_ratio);
        Self {
            bordered: false,
            area,
            title: Rect::new(
                workspace.x,
                workspace.y,
                workspace.width,
                workspace.height.min(1),
            ),
            workspace_body: Rect::new(
                workspace.x,
                workspace.y.saturating_add(WORKSPACE_HEADER_ROWS),
                workspace.width,
                workspace.height.saturating_sub(WORKSPACE_HEADER_ROWS + 1),
            ),
            agent_header: Rect::new(
                detail.x,
                detail.y.saturating_add(1),
                detail.width,
                u16::from(detail.height >= 2),
            ),
            agent_body: Rect::new(
                detail.x,
                detail.y.saturating_add(3),
                detail.width,
                detail.height.saturating_sub(3),
            ),
            footer: if footer && workspace.height > 0 {
                Rect::new(
                    workspace.x,
                    workspace.bottom().saturating_sub(1),
                    workspace.width,
                    1,
                )
            } else {
                Rect::default()
            },
            footer_split: None,
            section_divider: crate::ui::sidebar_section_divider_rect(area, split_ratio),
            toggle: Self::toggle_rect(area),
        }
    }

    fn bordered(area: Rect, split_ratio: f32, footer: bool) -> Self {
        let (_, detail) = crate::ui::expanded_sidebar_sections(area, split_ratio);
        let bottom_rows = if footer { 3 } else { 1 };
        // The section ratio is shared with the borderless layout; when the footer would
        // collide with the split, the split yields so the agent rule stays visible.
        let split_y = detail
            .y
            .min(area.bottom().saturating_sub(bottom_rows + 1))
            .max(area.y.saturating_add(2));
        let inner_x = area.x.saturating_add(1);
        let inner_width = area.width.saturating_sub(2);
        let agent_bottom = area.bottom().saturating_sub(bottom_rows);
        Self {
            bordered: true,
            area,
            title: Rect::new(area.x, area.y, area.width, 1),
            workspace_body: Rect::new(
                inner_x,
                area.y.saturating_add(1),
                inner_width,
                split_y.saturating_sub(area.y.saturating_add(1)),
            ),
            agent_header: Rect::new(area.x, split_y, area.width, 1),
            agent_body: Rect::new(
                inner_x,
                split_y.saturating_add(1),
                inner_width,
                agent_bottom.saturating_sub(split_y.saturating_add(1)),
            ),
            footer: if footer {
                Rect::new(inner_x, area.bottom().saturating_sub(2), inner_width, 1)
            } else {
                Rect::default()
            },
            footer_split: footer.then(|| area.bottom().saturating_sub(3)),
            section_divider: Rect::new(area.x, split_y, area.width.saturating_sub(1), 1),
            toggle: Self::toggle_rect(area),
        }
    }

    fn toggle_rect(area: Rect) -> Rect {
        Rect::new(
            area.right().saturating_sub(2),
            area.bottom().saturating_sub(1),
            u16::from(area.width > 1),
            u16::from(area.height > 0),
        )
    }

    /// The right-hand column doubles as the width drag handle in both layouts.
    pub(super) fn width_handle(&self) -> Rect {
        if self.area.is_empty() {
            Rect::default()
        } else {
            Rect::new(
                self.area.right().saturating_sub(1),
                self.area.y,
                1,
                self.area.height,
            )
        }
    }

    /// Fills the frame surface and draws the rules that carry no labels.
    pub(super) fn paint(&self, buffer: &mut Buffer, materials: &Materials) {
        let area = self.area;
        if area.is_empty() {
            return;
        }
        buffer.set_style(area, Style::default().bg(materials.frame));
        let rule = Style::default().fg(materials.border);
        if !self.bordered {
            let separator_x = area.right().saturating_sub(1);
            for y in area.y..area.bottom() {
                if let Some(cell) = buffer.cell_mut((separator_x, y)) {
                    cell.set_symbol("│").set_style(rule);
                }
            }
            if self.agent_header.height > 0 {
                let divider = self.agent_header.y.saturating_sub(1);
                put_text(
                    buffer,
                    self.agent_header.x,
                    divider,
                    self.agent_header.width,
                    &"─".repeat(self.agent_header.width as usize),
                    rule,
                );
            }
            return;
        }
        let right = area.right().saturating_sub(1);
        for y in area.y..area.bottom() {
            for x in [area.x, right] {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.set_symbol("│").set_style(rule);
                }
            }
        }
        if let Some(y) = self.footer_split {
            draw_rule(buffer, area, y, ("├", "┤"), rule);
        }
        draw_rule(
            buffer,
            area,
            area.bottom().saturating_sub(1),
            ("└", "┘"),
            rule,
        );
    }

    pub(super) fn render_title(
        &self,
        buffer: &mut Buffer,
        title: &str,
        right: Option<&str>,
        materials: &Materials,
        palette: &Palette,
    ) {
        label_row(
            buffer,
            self.title,
            self.bordered,
            ("┌", "┐"),
            title,
            right.map(|right| (right, label_style(palette))),
            materials,
            palette,
        );
    }

    /// Draws the agent panel label and returns the clickable right-hand label.
    pub(super) fn render_agent_header(
        &self,
        buffer: &mut Buffer,
        right: (&str, Style),
        materials: &Materials,
        palette: &Palette,
    ) -> Rect {
        label_row(
            buffer,
            self.agent_header,
            self.bordered,
            ("├", "┤"),
            "agents",
            Some(right),
            materials,
            palette,
        )
    }

    /// The section split stays draggable except where it carries a clickable label.
    pub(super) fn section_divider_excluding(&self, label: Rect) -> Rect {
        let divider = self.section_divider;
        if !self.bordered || label.is_empty() || label.y != divider.y {
            return divider;
        }
        Rect::new(
            divider.x,
            divider.y,
            label.x.saturating_sub(divider.x),
            divider.height,
        )
    }

    pub(super) fn render_toggle(&self, buffer: &mut Buffer, palette: &Palette) -> Rect {
        put_text(
            buffer,
            self.toggle.x,
            self.toggle.y,
            self.toggle.width,
            "«",
            Style::default().fg(palette.overlay0),
        );
        self.toggle
    }

    /// Draws `+ space`, the session picker entry and the global menu launcher.
    pub(super) fn render_footer(
        &self,
        buffer: &mut Buffer,
        actions: FooterActions,
        palette: &Palette,
        hits: &mut ShellHitMap,
    ) {
        let row = self.footer;
        if row.is_empty() {
            return;
        }
        let quiet = Style::default().fg(palette.overlay0);
        let content = Rect::new(
            row.x.saturating_add(1),
            row.y,
            row.width.saturating_sub(2),
            1,
        );
        let new_label = "+ space";
        let new_width = display_width(new_label).min(content.width);
        put_text(buffer, content.x, row.y, new_width, new_label, quiet);
        hits.new_workspace = Rect::new(content.x, row.y, new_width, 1);

        let sessions_width = display_width("sessions");
        let full_menu = display_width("≡ menu");
        let room = content.width.saturating_sub(new_width);
        // Narrow combined windows keep the session entry and shorten the launcher to its
        // glyph; one space separates every entry.
        let fits = |menu: u16| room >= menu + sessions_width + 2;
        let show_sessions = actions.sessions && fits(1);
        let menu_width = if !show_sessions || fits(full_menu) {
            full_menu
        } else {
            1
        }
        .min(room);
        let menu = Rect::new(
            content.right().saturating_sub(menu_width),
            row.y,
            menu_width,
            1,
        );
        if menu_width > 0 {
            let (glyph, glyph_style) = if actions.attention {
                (
                    "●",
                    Style::default()
                        .fg(palette.accent)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ("≡", quiet)
            };
            put_text(buffer, menu.x, row.y, 1, glyph, glyph_style);
            put_text(
                buffer,
                menu.x.saturating_add(2),
                row.y,
                menu.width.saturating_sub(2),
                "menu",
                quiet,
            );
        }
        hits.global_launcher = menu;

        let gap_start = content.x.saturating_add(new_width);
        let gap = menu.x.saturating_sub(gap_start);
        hits.session_picker = if show_sessions && gap >= sessions_width + 2 {
            let x = gap_start + (gap - sessions_width) / 2;
            put_text(buffer, x, row.y, sessions_width, "sessions", quiet);
            Rect::new(x, row.y, sessions_width, 1)
        } else {
            Rect::default()
        };
    }
}

pub(super) fn label_style(palette: &Palette) -> Style {
    Style::default()
        .fg(palette.subtext0)
        .add_modifier(Modifier::BOLD)
}

/// Paints a row's interaction state over everything drawn in it, connectors included.
pub(super) fn paint_row(buffer: &mut Buffer, rect: Rect, tone: RowTone, materials: &Materials) {
    let rect = rect.intersection(buffer.area);
    let (bg, fg) = match tone {
        RowTone::Plain => return,
        RowTone::Focused => (materials.focused, None),
        RowTone::Dragged => (materials.hover, None),
        RowTone::Selected => (materials.selected_bg, Some(materials.selected_fg)),
    };
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let cell = &mut buffer[(x, y)];
            cell.set_bg(bg);
            if let Some(fg) = fg {
                cell.set_fg(fg);
            }
        }
    }
}

fn draw_rule(buffer: &mut Buffer, area: Rect, y: u16, caps: (&str, &str), style: Style) {
    if area.width < 2 || y < area.y || y >= area.bottom() {
        return;
    }
    let fill = area.width.saturating_sub(2) as usize;
    put_text(buffer, area.x, y, 1, caps.0, style);
    put_text(
        buffer,
        area.x.saturating_add(1),
        y,
        area.width.saturating_sub(2),
        &"─".repeat(fill),
        style,
    );
    put_text(buffer, area.right().saturating_sub(1), y, 1, caps.1, style);
}

/// `┌─ left ───── right ┐` when bordered, ` left        right` otherwise.
fn label_row(
    buffer: &mut Buffer,
    row: Rect,
    bordered: bool,
    caps: (&str, &str),
    left: &str,
    right: Option<(&str, Style)>,
    materials: &Materials,
    palette: &Palette,
) -> Rect {
    if row.is_empty() {
        return Rect::default();
    }
    let label = label_style(palette);
    if !bordered {
        put_text(buffer, row.x, row.y, row.width, &format!(" {left}"), label);
        let Some((text, style)) = right else {
            return Rect::default();
        };
        let width = display_width(text).min(row.width);
        put_right_text(buffer, row, row.y, text, style);
        return Rect::new(row.right().saturating_sub(width), row.y, width, 1);
    }
    let rule = Style::default().fg(materials.border);
    draw_rule(buffer, row, row.y, caps, rule);
    // Inside the caps: "─ left " on the left and " right " against the right cap.
    let inner = row.width.saturating_sub(2);
    let left_width = display_width(left);
    let left_room = inner.saturating_sub(3);
    let left_shown = left_width.min(left_room);
    if left_shown > 0 {
        put_text(buffer, row.x.saturating_add(2), row.y, 1, " ", rule);
        put_text(
            buffer,
            row.x.saturating_add(3),
            row.y,
            left_shown,
            left,
            label,
        );
        put_text(
            buffer,
            row.x.saturating_add(3 + left_shown),
            row.y,
            1,
            " ",
            rule,
        );
    }
    let Some((text, style)) = right else {
        return Rect::default();
    };
    let width = display_width(text);
    let used = if left_shown > 0 { left_shown + 3 } else { 0 };
    if width == 0 || used + width + 3 > inner {
        return Rect::default();
    }
    let x = row.right().saturating_sub(width + 2);
    put_text(buffer, x.saturating_sub(1), row.y, 1, " ", rule);
    put_text(buffer, x, row.y, width, text, style);
    put_text(buffer, x.saturating_add(width), row.y, 1, " ", rule);
    Rect::new(x, row.y, width, 1)
}
