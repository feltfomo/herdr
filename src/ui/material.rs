use ratatui::style::Color;

use crate::app::state::Palette;

/// Surface colors for the bordered sidebar and floating pickers.
///
/// Tinted surfaces are sRGB mixes of the accent into the theme's own surfaces, so a
/// wallpaper-generated palette retints every surface by changing only its tokens. Mixing
/// needs RGB on both sides; a theme built from terminal color names (or `reset` to keep
/// the terminal background) falls back to its plain tokens instead of guessing values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Materials {
    pub frame: Color,
    pub raised: Color,
    pub field: Color,
    pub hover: Color,
    pub focused: Color,
    pub selected_bg: Color,
    pub selected_fg: Color,
    pub border: Color,
    pub strong_border: Color,
}

impl Materials {
    pub(crate) fn from_palette(palette: &Palette) -> Self {
        let accent = palette.accent;
        let frame = mix(accent, palette.sidebar_bg, 0.08).unwrap_or(palette.sidebar_bg);
        let raised = mix(accent, palette.panel_bg, 0.06).unwrap_or(palette.panel_bg);
        // Palettes have no outline-variant token; overlay0 pulled toward the base gives the
        // low-contrast hairline the reference borders are mixed from.
        let hairline = mix(palette.overlay0, raised, 0.35);
        Self {
            frame,
            raised,
            field: mix(accent, palette.active_row_bg, 0.04).unwrap_or(palette.surface0),
            hover: mix(accent, frame, 0.12).unwrap_or(palette.surface1),
            focused: mix(accent, frame, 0.05).unwrap_or(palette.active_row_bg),
            selected_bg: accent,
            selected_fg: palette.accent_ink(),
            border: hairline
                .and_then(|line| mix(accent, line, 0.35))
                .unwrap_or(palette.overlay0),
            strong_border: hairline
                .and_then(|line| mix(accent, line, 0.65))
                .unwrap_or(accent),
        }
    }
}

/// `amount` of `top` over `1 - amount` of `base`, or `None` unless both are RGB.
fn mix(top: Color, base: Color, amount: f32) -> Option<Color> {
    let (Color::Rgb(tr, tg, tb), Color::Rgb(br, bg, bb)) = (top, base) else {
        return None;
    };
    let channel =
        |t: u8, b: u8| (f32::from(t) * amount + f32::from(b) * (1.0 - amount)).round() as u8;
    Some(Color::Rgb(
        channel(tr, br),
        channel(tg, bg),
        channel(tb, bb),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wallpaper_palette() -> Palette {
        let mut palette = Palette::catppuccin();
        palette.accent = Color::Rgb(0xb0, 0xca, 0xdb);
        palette.sidebar_bg = Color::Rgb(0x12, 0x13, 0x14);
        palette.panel_bg = Color::Rgb(0x12, 0x13, 0x14);
        palette
    }

    #[test]
    fn rgb_palettes_tint_surfaces_toward_the_accent() {
        let materials = Materials::from_palette(&wallpaper_palette());
        // 8% of #b0cadb over #121314.
        assert_eq!(materials.frame, Color::Rgb(0x1f, 0x22, 0x24));
        assert_eq!(materials.selected_bg, Color::Rgb(0xb0, 0xca, 0xdb));
        let Color::Rgb(_, hover_g, _) = materials.hover else {
            panic!("hover should stay RGB");
        };
        let Color::Rgb(_, frame_g, _) = materials.frame else {
            panic!("frame should stay RGB");
        };
        assert!(hover_g > frame_g, "hover must read lighter than the frame");
    }

    #[test]
    fn reset_backgrounds_keep_the_terminal_background() {
        let palette = Palette::terminal();
        let materials = Materials::from_palette(&palette);
        assert_eq!(materials.frame, Color::Reset);
        assert_eq!(materials.raised, palette.panel_bg);
        assert_eq!(materials.border, palette.overlay0);
        assert_eq!(materials.strong_border, palette.accent);
    }

    #[test]
    fn accent_ink_contrasts_with_pale_and_dark_accents_unless_overridden() {
        let mut palette = wallpaper_palette();
        let Color::Rgb(r, g, b) = palette.accent_ink() else {
            panic!("derived ink should be RGB");
        };
        assert!(
            u16::from(r) + u16::from(g) + u16::from(b) < 200,
            "pale accent needs dark ink"
        );

        palette.accent = Color::Rgb(0x1e, 0x3a, 0x5f);
        let Color::Rgb(r, _, _) = palette.accent_ink() else {
            panic!("derived ink should be RGB");
        };
        assert!(r > 200, "dark accent needs pale ink");

        palette.on_accent = Some(Color::Rgb(0x1a, 0x33, 0x40));
        assert_eq!(palette.accent_ink(), Color::Rgb(0x1a, 0x33, 0x40));
        assert_eq!(
            Materials::from_palette(&palette).selected_fg,
            Color::Rgb(0x1a, 0x33, 0x40)
        );
    }
}
