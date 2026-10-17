//! Split finalized assistant output into plain and ```` ```md ```` fenced
//! segments, rendering the fenced content as rich Markdown.
//!
//! Assistant messages arrive as a single plain-text string (the stream's
//! `Text` content blocks are not typed per-fence), so this module does the
//! fenced-block extraction that the transcript render path needs.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::palette::Palette;
use crate::plugin::builtin::markdown::renderer::render_markdown;

/// Language tag that opens a Markdown block: ```` ```md ````.
const MD_FENCE: &str = "md";

/// Render one assistant message that contains at least one ```` ```md ````
/// fenced block.
///
/// Text outside a fence is emitted verbatim in the agent's `secondary`
/// colour; fenced content is rendered by [`render_markdown`]. `prefix` (the
/// localized agent bullet, e.g. `"Agent: "`) is bolded and prepended to the
/// first emitted line, mirroring `crate::ui::line_block`'s layout on the
/// no-fence path.
///
/// The fence content is rendered at `width - prefix` columns so the bolded
/// bullet never pushes the first line past the transcript width. A fence
/// that is never closed (possible mid-stream, though this only runs on
/// finalized text) renders its buffered content rather than dropping it.
#[must_use]
pub fn render_assistant_text(
    text: &str,
    prefix: &str,
    width: usize,
    palette: &Palette,
) -> Vec<Line<'static>> {
    let base = palette.base_style().fg(palette.secondary);
    let content_width = width.saturating_sub(prefix.chars().count()).max(1);

    let mut output: Vec<Line<'static>> = Vec::new();
    let mut in_block = false;
    let mut buffer = String::new();

    for line in text.lines() {
        if let Some(info) = line.trim_start().strip_prefix("```") {
            if in_block {
                // Closing fence: flush the buffered Markdown.
                output.append(&mut render_markdown(&buffer, palette, content_width));
                buffer.clear();
                in_block = false;
            } else if info.trim() == MD_FENCE {
                in_block = true;
                buffer.clear();
            } else {
                // Some other fence (a code block, say) — not ours to render;
                // keep the line verbatim.
                output.push(plain(line, base));
            }
            continue;
        }

        if in_block {
            buffer.push_str(line);
            buffer.push('\n');
        } else {
            output.push(plain(line, base));
        }
    }

    if in_block && !buffer.is_empty() {
        output.append(&mut render_markdown(&buffer, palette, content_width));
    }

    if output.is_empty() {
        return output;
    }

    // Bold the agent bullet onto the first line only, preserving whatever
    // style/alignment that line already carried.
    let bullet = Span::styled(prefix.to_string(), base.add_modifier(Modifier::BOLD));
    let first = output.remove(0);
    let mut spans = Vec::with_capacity(first.spans.len() + 1);
    spans.push(bullet);
    spans.extend(first.spans);
    let mut line = Line::from(spans);
    line.style = first.style;
    line.alignment = first.alignment;
    output.insert(0, line);

    output
}

/// A verbatim, unstyled-content line carrying the agent text colour.
fn plain(text: &str, base: Style) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::builtin::themes::catalog::Theme;

    fn palette() -> Palette {
        Palette::for_theme(Theme::Dark)
    }

    fn flat(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn fences_are_stripped_and_content_rendered() {
        let text = "Here is a table:\n```md\n# Heading\n\ntext\n```\ndone";
        let lines = render_assistant_text(text, "Agent: ", 80, &palette());
        let out = flat(&lines);

        assert!(!out.contains("```"), "fence markers should be removed");
        assert!(out.contains("Here is a table:"));
        assert!(out.contains("Heading"));
        assert!(out.contains("done"));
    }

    #[test]
    fn agent_prefix_lands_on_the_first_line_only() {
        let text = "intro\n```md\nbody\n```";
        let lines = render_assistant_text(text, "Agent: ", 80, &palette());

        let first = flat(&lines[..1]);
        assert!(first.starts_with("Agent: "));
        // The bullet appears exactly once across the whole render.
        let occurrences = flat(&lines).matches("Agent: ").count();
        assert_eq!(occurrences, 1);
    }

    #[test]
    fn unterminated_fence_still_renders_buffered_content() {
        let lines = render_assistant_text("```md\nhello", "Agent: ", 80, &palette());
        assert!(flat(&lines).contains("hello"));
    }

    #[test]
    fn non_md_fences_are_left_verbatim() {
        let lines = render_assistant_text("```rust\nlet x = 1;\n```", "Agent: ", 80, &palette());
        let out = flat(&lines);
        assert!(out.contains("```rust"));
        assert!(out.contains("let x = 1;"));
    }

    #[test]
    fn empty_message_renders_no_lines() {
        assert!(render_assistant_text("", "Agent: ", 80, &palette()).is_empty());
    }
}
