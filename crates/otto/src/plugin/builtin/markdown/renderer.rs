//! Markdown rendering via [`ratskin`] (a [`termimad`] wrapper).
//!
//! Turns the contents of a ```` ```md ```` fenced block into styled
//! ratatui [`Line`]s. `ratskin` targets ratatui 0.30 — the same version
//! this crate uses — so its `Line`/`Span` types drop straight into the
//! transcript with no cross-version colour remapping (unlike the
//! `internal:changelog` screen, which massages colours across
//! `tui-markdown`'s ratatui 0.29 boundary).
//!
//! `ratskin` was chosen over `limner`/`tui-markdown` because both of those
//! silently drop GFM tables — the plugin's headline use case. `termimad`
//! renders tables (box-drawing, per-cell layout) and everything else.
//!
//! [`termimad`]'s stock [`MadSkin`] uses fixed grey levels, so [`skin_for`]
//! rebuilds it from the active [`Palette`] to keep rendered Markdown
//! consistent with the rest of the TUI.

use ratatui::style::Color as RatColor;
use ratatui::text::Line;
use ratskin::RatSkin;
use termimad::crossterm::style::Color as CtColor;
use termimad::{Alignment, MadSkin};

use crate::palette::Palette;

/// Build a [`RatSkin`] from the active [`Palette`].
///
/// Every colour-bearing slot is mapped to a palette colour; heading
/// alignment is forced left (termimad centres H1 by default, which reads
/// oddly inside a chat transcript).
#[must_use]
pub fn skin_for(palette: &Palette) -> RatSkin {
    let mut skin = MadSkin::default();

    skin.paragraph.set_fg(ct(palette.fg));
    skin.bold.set_fg(ct(palette.fg));
    skin.italic.set_fg(ct(palette.fg));
    skin.strikeout.set_fg(ct(palette.fg));
    skin.inline_code
        .set_fgbg(ct(palette.warning), ct(palette.bg));
    skin.code_block
        .set_fgbg(ct(palette.success), ct(palette.bg));
    for header in &mut skin.headers {
        header.set_fg(ct(palette.accent));
        header.align = Alignment::Left;
    }
    skin.table.set_fg(ct(palette.border));
    skin.bullet.set_fg(ct(palette.accent));
    skin.quote_mark.set_fg(ct(palette.muted));
    skin.horizontal_rule.set_fg(ct(palette.border));

    RatSkin { skin }
}

/// Render `input` as rich Markdown, wrapping to `width` columns.
///
/// `width` bounds table and paragraph layout; pass the transcript's inner
/// content width. Returns an empty vector for blank input (so an empty
/// fence does not emit a stray blank line).
#[must_use]
pub fn render_markdown(input: &str, palette: &Palette, width: usize) -> Vec<Line<'static>> {
    if input.trim().is_empty() {
        return Vec::new();
    }
    // termimad takes a u16 column count; clamp so a huge or zero width
    // (e.g. a squeezed layout mid-resize) can't overflow or panic.
    let width = width.clamp(1, u16::MAX as usize) as u16;
    skin_for(palette).parse(RatSkin::parse_text(input), width)
}

/// Map a ratatui [`RatColor`] to the equivalent crossterm colour.
///
/// The two crates name their "bright" and "dark" ANSI variants oppositely:
/// ratatui's `Red` is crossterm's `DarkRed`, and ratatui's `LightRed` is
/// crossterm's `Red`. Getting this wrong would double-brighten every slot.
fn ct(color: RatColor) -> CtColor {
    match color {
        RatColor::Reset => CtColor::Reset,
        RatColor::Black => CtColor::Black,
        RatColor::Red => CtColor::DarkRed,
        RatColor::Green => CtColor::DarkGreen,
        RatColor::Yellow => CtColor::DarkYellow,
        RatColor::Blue => CtColor::DarkBlue,
        RatColor::Magenta => CtColor::DarkMagenta,
        RatColor::Cyan => CtColor::DarkCyan,
        RatColor::Gray => CtColor::Grey,
        RatColor::DarkGray => CtColor::DarkGrey,
        RatColor::LightRed => CtColor::Red,
        RatColor::LightGreen => CtColor::Green,
        RatColor::LightYellow => CtColor::Yellow,
        RatColor::LightBlue => CtColor::Blue,
        RatColor::LightMagenta => CtColor::Magenta,
        RatColor::LightCyan => CtColor::Cyan,
        RatColor::White => CtColor::White,
        RatColor::Rgb(r, g, b) => CtColor::Rgb { r, g, b },
        RatColor::Indexed(i) => CtColor::AnsiValue(i),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::builtin::themes::catalog::Theme;
    use ratatui::style::Modifier;

    fn palette() -> Palette {
        Palette::for_theme(Theme::Dark)
    }

    /// Concatenate every span's text across `lines`, for content asserts.
    fn flat(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn blank_input_renders_nothing() {
        assert!(render_markdown("   \n  ", &palette(), 80).is_empty());
    }

    #[test]
    fn heading_renders_bold_accent_text() {
        let lines = render_markdown("# Title", &palette(), 80);
        let span = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content == "Title")
            .expect("heading text span");
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        // The palette accent is applied (exact round-trip through crossterm).
        assert_eq!(span.style.fg, Some(palette().accent));
    }

    #[test]
    fn paragraph_uses_palette_foreground() {
        let lines = render_markdown("plain text", &palette(), 80);
        let span = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains("plain"))
            .expect("paragraph span");
        assert_eq!(span.style.fg, Some(palette().fg));
    }

    #[test]
    fn preserves_bold_marker_semantics() {
        let lines = render_markdown("this is **bold** text", &palette(), 80);
        assert!(flat(&lines).contains("this is bold text"));
        let bold = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content == "bold")
            .expect("bold span");
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn table_renders_every_cell_on_separate_rows() {
        let md = "| Name | Value |\n|------|-------|\n| one  | 1     |";
        let lines = render_markdown(md, &palette(), 80);
        assert!(lines.len() >= 3, "table should span at least 3 rows");
        let text = flat(&lines);
        for cell in ["Name", "Value", "one", "1"] {
            assert!(text.contains(cell), "missing table cell {cell:?}");
        }
        // Cells from the same row must not share a line with other rows.
        let header = flat(&lines[..1]);
        assert!(header.contains("Name") && header.contains("Value"));
        assert!(!header.contains("one"));
    }

    #[test]
    fn lines_fit_requested_width() {
        let md = "| a very long column header | another long column header |\n|---|---|\n| x | y |";
        for line in render_markdown(md, &palette(), 30) {
            assert!(line.width() <= 30, "line exceeded width: {:?}", line);
        }
    }
}
