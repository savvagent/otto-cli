//! Render pass: paint the current [`App`] state into the frame.

use crate::app::{App, Entry, InputMode, TranscriptEntry, log_scroll_y};
use crate::palette::Palette;
use crate::providers::{
    PROVIDER_SELECTOR_DISCOVERABILITY_THRESHOLD, ProviderSpec, effective_providers,
};
use crate::splash;
use otto_host::ToolCallStatus;
use otto_plugin::ContentBlockId;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, FrameExt, List, ListItem, Padding, Paragraph, Widget,
        Wrap,
    },
};
use tui_spinner::CircleSpinner;

/// Rows reserved in the conversation paragraph for each `Entry::Canvas`
/// placeholder. The image (or source-code fallback) is overlaid on top of
/// these blank rows after the paragraph renders. Phase 1 uses a fixed
/// height; Phase 2 will measure document height from the renderer.
const CANVAS_RESERVED_ROWS: u16 = 12;

/// Pre-rendered styled spans for a single `Entry::Tool` row, produced during
/// `compute_home_frame_data`. Stored on `HomeFrameData` and consumed by the
/// sync `render_log` so the render path never locks plugin mutexes.
#[derive(Debug, Clone)]
pub struct ToolEntryRender {
    /// Spans for the arguments line.
    pub arg_spans: Vec<otto_plugin::StyledSpan>,
    /// Spans for the result line; `None` while the tool call is in flight.
    pub result_spans: Option<Vec<otto_plugin::StyledSpan>>,
}

/// Pre-computed plugin slot output for one render frame. Built async from
/// `compute_home_frame_data` before `terminal.draw` runs so the draw closure
/// stays synchronous and never touches plugin mutexes.
pub struct HomeFrameData {
    pub banner: Vec<otto_plugin::StyledLine>,
    pub tips: Vec<otto_plugin::StyledLine>,
    pub footer_left: Vec<otto_plugin::StyledLine>,
    pub footer_center: Vec<otto_plugin::StyledLine>,
    pub footer_center_turn_line: Option<usize>,
    pub footer_right: Vec<otto_plugin::StyledLine>,
    /// One entry per `Entry::Tool` in `app.entries`, in order. The Nth
    /// `Entry::Tool` encountered while iterating `app.entries` maps to the
    /// Nth element of this vec.
    pub tool_entries: Vec<ToolEntryRender>,
}

impl HomeFrameData {
    /// Empty fallback used when plugins are not installed yet.
    pub fn empty() -> Self {
        Self {
            banner: vec![],
            tips: vec![],
            footer_left: vec![],
            footer_center: vec![],
            footer_center_turn_line: None,
            footer_right: vec![],
            tool_entries: vec![],
        }
    }
}

/// Resolve every slot's lines for the current frame. Locks plugin mutexes
/// briefly per contributor.
pub async fn compute_home_frame_data(app: &crate::app::App, area: Rect) -> HomeFrameData {
    use std::sync::Once;

    use crate::plugin::convert::rect_to_region;
    use crate::plugin::slots::SlotRouter;

    static WARNED_NO_RUNTIME: Once = Once::new();

    let (Some(reg), Some(idx)) = (
        app.plugin_registry.as_ref().cloned(),
        app.plugin_indexes.as_ref().cloned(),
    ) else {
        WARNED_NO_RUNTIME.call_once(|| {
            tracing::warn!(
                "compute_home_frame_data called before install_plugin_runtime — TUI is rendering with no plugin output"
            );
        });
        return HomeFrameData::empty();
    };
    let reg_guard = reg.read().await;
    let idx_guard = idx.read().await;
    let router = SlotRouter::new(&idx_guard, &reg_guard);

    let full_row = rect_to_region(Rect::new(area.x, area.y, area.width, 1));
    let banner = router.render("home.banner", full_row).await;
    let tips = router.render("home.tips", full_row).await;
    let footer_left = router.render("home.footer.left", full_row).await;
    let (footer_center, footer_center_turn_line) =
        render_footer_center_slot(&router, full_row).await;
    let footer_right = router.render("home.footer.right", full_row).await;

    // The registry and index read-locks (`reg_guard`/`idx_guard`) are
    // dropped here so write-lock waiters (e.g. `/connect` while a screen
    // is open) are not blocked by the tool-summary loop.
    drop(reg_guard);
    drop(idx_guard);

    let tool_entries = compute_tool_entries(
        &app.entries,
        app.plugin_indexes.as_ref().cloned(),
        app.plugin_registry.as_ref().cloned(),
    )
    .await;

    HomeFrameData {
        banner,
        tips,
        footer_left,
        footer_center,
        footer_center_turn_line,
        footer_right,
        tool_entries,
    }
}

async fn render_footer_center_slot(
    router: &crate::plugin::slots::SlotRouter<'_>,
    region: otto_plugin::Region,
) -> (Vec<otto_plugin::StyledLine>, Option<usize>) {
    let mut out = Vec::new();
    let mut turn_line_idx = None;
    for pid in router.contributors("home.footer.center") {
        let Some(handle) = router.registry.get(pid) else {
            tracing::error!(
                plugin_id = %pid.as_str(),
                slot_id = "home.footer.center",
                "contributor in slot index but missing from registry — index/registry divergence"
            );
            continue;
        };
        let Ok(plugin) = handle.try_lock() else {
            tracing::trace!(
                plugin_id = %pid.as_str(),
                slot_id = "home.footer.center",
                "slot contributor busy; skipping for this frame"
            );
            continue;
        };
        let rendered = plugin.render_slot("home.footer.center", region);
        if pid.as_str() == "internal:home-footer" {
            turn_line_idx = rendered
                .iter()
                .position(|line| !line.spans.is_empty())
                .map(|idx| out.len() + idx);
        }
        out.extend(rendered);
    }
    (out, turn_line_idx)
}

/// Compute `ToolEntryRender`s for every `Entry::Tool` in `entries` by
/// asking the plugin registry for its tool-summary router. Passed cloned
/// `Arc<RwLock<...>>` handles instead of `&RwLockReadGuard` so the
/// caller (compute_home_frame_data) can drop its guards before this
/// long-running loop. No-op when either handle is `None` (e.g. plugin
/// runtime not installed).
async fn compute_tool_entries(
    entries: &[Entry],
    plugin_indexes: Option<std::sync::Arc<tokio::sync::RwLock<crate::plugin::manifests::Indexes>>>,
    plugin_registry: Option<
        std::sync::Arc<tokio::sync::RwLock<crate::plugin::registry::PluginRegistry>>,
    >,
) -> Vec<ToolEntryRender> {
    let (Some(reg_handle), Some(idx_handle)) = (plugin_registry, plugin_indexes) else {
        return Vec::new();
    };
    let tool_router = crate::plugin::tool_summaries::ToolSummaryRouter::new(
        idx_handle.clone(),
        reg_handle.clone(),
    );
    let mut tool_entries: Vec<ToolEntryRender> = Vec::new();
    for entry in entries {
        let crate::app::Entry::Tool {
            name,
            args,
            result_text,
            ..
        } = entry
        else {
            continue;
        };
        let arg_spans = match tool_router.summarize_call(name, args).await {
            Some(spans) => spans,
            None => otto_plugin::styled::json_spans(args),
        };
        let result_spans = match result_text {
            None => None,
            Some(text) => {
                let spans = match tool_router.summarize_result(name, text).await {
                    Some(spans) => spans,
                    None => match serde_json::from_str::<serde_json::Value>(text) {
                        Ok(v) => otto_plugin::styled::json_spans(&v),
                        Err(_) => vec![otto_plugin::StyledSpan::muted(text.clone())],
                    },
                };
                Some(spans)
            }
        };
        tool_entries.push(ToolEntryRender {
            arg_spans,
            result_spans,
        });
    }
    tool_entries
}

pub fn render(
    app: &mut App,
    frame: &mut Frame,
    frame_data: &HomeFrameData,
    tick: u64,
    active_turn_id: Option<u32>,
    pending_turn_id: Option<u32>,
) {
    let area = frame.area();

    if app.show_splash {
        splash::render(frame, area, &app.splash_sandbox);
        return;
    }

    let palette = Palette::for_theme(app.active_theme);

    // Paint the active theme's base style across the whole frame so any
    // widget that doesn't set its own bg picks up the theme background.
    frame.buffer_mut().set_style(area, palette.base_style());

    // Build the prompt textarea up-front so we can ask tui-textarea how
    // tall it wants to be (driven by wrap mode + min/max rows configured
    // on `app.input_textarea`). The measured height drives the input
    // constraint below so the box grows with multi-line / wrapped input
    // and shrinks back to its 3-row minimum when cleared.
    let mut textarea = app.input_textarea.clone();
    let prompt_block_value = prompt_block(palette);
    textarea.set_block(prompt_block_value.clone());
    textarea.set_style(palette.base_style());
    // tui-textarea defaults the cursor-line style to UNDERLINED, which
    // ends up underlining the whole one-line prompt. Override to the
    // base style so the input renders flat like the rest of the UI.
    textarea.set_cursor_line_style(palette.base_style());
    let input_rows = textarea.measure(area.width).preferred_rows;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),          // header
            Constraint::Min(1),             // log
            Constraint::Length(1),          // banner (plugin slot: home.banner)
            Constraint::Length(1),          // tips (plugin slot: home.tips)
            Constraint::Length(input_rows), // input (dynamic, clamped by textarea min/max rows)
            Constraint::Length(1),          // footer (plugin slots: home.footer.*)
        ])
        .split(area);

    let resumed_label = app
        .resumed_at
        .as_deref()
        .map(|ts| format!(" · resumed: {ts}"))
        .unwrap_or_default();
    let header_text = if app.connected {
        format!(
            "Otto — {} · {}{}",
            app.active_provider_id.unwrap_or("?"),
            app.model,
            resumed_label,
        )
    } else {
        "Otto — disconnected · type /connect".to_string()
    };
    let header_color = if app.connected {
        palette.accent
    } else {
        palette.warning
    };
    let header = Paragraph::new(header_text)
        .style(
            palette
                .base_style()
                .fg(header_color)
                .add_modifier(Modifier::BOLD),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(palette.border).bg(palette.bg))
                .padding(Padding::horizontal(2)),
        );
    frame.render_widget(header, chunks[0]);

    let canvas_overlays = render_log(app, frame, chunks[1], palette, frame_data);
    // Persist each canvas's on-screen cell rect for the mouse handler in
    // `main.rs::run_app`, which hit-tests clicks outside the render pass.
    // Refreshed every frame so stale rects (e.g. after scroll) never route
    // a click into the wrong block.
    app.canvas_click_targets = canvas_overlays.iter().map(|o| (o.id, o.area)).collect();
    // After the conversation log paints, overlay any Entry::Canvas blocks
    // (image protocol when supported, source-code fallback otherwise).
    // `canvas_overlays` carries each placeholder's on-screen rect (already
    // clipped to the conversation block's inner area).
    if !canvas_overlays.is_empty() {
        render_canvas_overlays(app, frame, palette, &canvas_overlays);
    }

    // Banner row — one-line update banner, rendered from plugin slot.
    // The slot returns nothing when there is no update available, so the
    // row paints as theme background only.
    let banner_lines: Vec<Line<'static>> = frame_data
        .banner
        .iter()
        .cloned()
        .map(|l| crate::plugin::convert::styled_line_to_ratatui(l, &palette))
        .collect();
    let banner_para = Paragraph::new(banner_lines).style(palette.base_style());
    frame.render_widget(banner_para, chunks[2]);

    // Tips row — one-line hints above the prompt, rendered from plugin slot.
    // Inset horizontally so the row aligns with the content inside the
    // bordered blocks above and below (border + interior padding = 3 cols).
    let tips_lines: Vec<Line<'static>> = frame_data
        .tips
        .iter()
        .cloned()
        .map(|l| crate::plugin::convert::styled_line_to_ratatui(l, &palette))
        .collect();
    let tips_para = Paragraph::new(tips_lines).style(palette.base_style());
    frame.render_widget(tips_para, chunks[3]);

    frame.render_widget(&textarea, chunks[4]);

    // Ghost-completion overlay: the remainder of the highlighted palette
    // row's name, painted dim immediately after the cursor. Advisory only
    // — never written into `textarea`'s real, submittable buffer. See
    // `paint_ghost_completion` for the safety conditions. The screen
    // derives its answer from `prompt_line` (the textarea's own current
    // content), not from its own internal state — see
    // `Screen::ghost_completion`'s doc comment.
    let prompt_line = textarea.lines().first().map(String::as_str).unwrap_or("");
    if let Some((top_screen, _)) = app.screen_stack.top() {
        if let Some(ghost) = top_screen.ghost_completion(prompt_line) {
            paint_ghost_completion(
                frame.buffer_mut(),
                prompt_block_value.inner(chunks[4]),
                textarea.cursor(),
                textarea.lines(),
                &ghost,
                palette.base_style().fg(palette.muted),
            );
        }
    }

    // Footer row — see `compose_footer_line` for the join semantics.
    let separator = otto_plugin::StyledSpan::muted(" · ");
    let footer_center = footer_center_lines(
        &frame_data.footer_center,
        frame_data.footer_center_turn_line,
        active_turn_id,
        pending_turn_id,
        tick,
        palette,
    );
    let footer_left: Vec<Line<'static>> = frame_data
        .footer_left
        .iter()
        .cloned()
        .map(|line| crate::plugin::convert::styled_line_to_ratatui(line, &palette))
        .collect();
    let footer_right: Vec<Line<'static>> = frame_data
        .footer_right
        .iter()
        .cloned()
        .map(|line| crate::plugin::convert::styled_line_to_ratatui(line, &palette))
        .collect();
    let footer_line = compose_footer_ratatui_line(
        [&footer_left, &footer_center, &footer_right],
        &crate::plugin::convert::styled_span_to_ratatui(separator, &palette),
    );
    frame.render_widget(
        Paragraph::new(footer_line).style(palette.base_style()),
        chunks[5],
    );

    if app.is_file_picker_active {
        let popup = centered_rect(60, 40, area);
        frame.render_widget(Clear, popup);
        frame.render_widget_ref(app.file_explorer.widget(), popup);
    }

    // Screen-stack: if any screen is on top, paint it over the home chrome.
    if let Some((top_screen, layout)) = app.screen_stack.top() {
        let top_screen_id = app
            .screen_stack
            .top_id()
            .expect("top screen id present when top screen exists");
        paint_screen(
            frame,
            area,
            chunks[4].y,
            ActiveScreen {
                id: top_screen_id,
                screen: top_screen,
                layout,
            },
            palette,
            &app.splash_sandbox,
        );
    }

    if matches!(app.input_mode, InputMode::SelectingProvider) {
        let popup = centered_rect(60, 40, area);
        frame.render_widget(Clear, popup);
        let view = build_provider_selector_view(app, effective_providers().len());
        let items: Vec<ListItem> = if let Some(empty_state) = view.empty_state.as_ref() {
            vec![ListItem::new(Line::from(vec![Span::styled(
                empty_state.clone(),
                palette.base_style().fg(palette.muted),
            )]))]
        } else {
            view.items
                .iter()
                .map(|item| {
                    let style = if item.is_selected {
                        palette
                            .base_style()
                            .fg(palette.accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        palette.base_style()
                    };
                    let active_marker = if item.is_active { " (active)" } else { "" };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{:<22}", item.spec.display_name), style),
                        Span::styled(
                            format!(" {}{}", item.spec.id, active_marker),
                            palette.base_style().fg(palette.muted),
                        ),
                    ]))
                })
                .collect()
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette.border).bg(palette.bg))
            .padding(Padding::new(2, 2, 1, 0))
            .title(Line::styled(
                " Connect to provider ",
                palette.base_style().fg(palette.fg),
            ))
            .title_bottom(Line::from(view.help_text).right_aligned());
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        let (query_area, list_area) = if view.show_query_row {
            let sections = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1)])
                .split(inner);
            (sections[0], sections[1])
        } else {
            (Rect::default(), inner)
        };
        if let Some(query_text) = view.query_text.as_ref() {
            let query_style = if view.query_is_placeholder {
                palette.base_style().fg(palette.muted)
            } else {
                palette.base_style().fg(palette.fg)
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("Search: ", palette.base_style().fg(palette.muted)),
                    Span::styled(query_text.clone(), query_style),
                ]))
                .style(palette.base_style()),
                query_area,
            );
        }
        let list = List::new(items)
            .style(palette.base_style())
            .highlight_symbol("> ");
        frame.render_widget(list, list_area);
    }

    if matches!(app.input_mode, InputMode::PermissionPrompt) {
        if let Some(req) = &app.pending_permission {
            let popup = centered_rect(60, 40, area);
            frame.render_widget(Clear, popup);

            let args_pretty =
                serde_json::to_string_pretty(&req.args).unwrap_or_else(|_| req.args.to_string());
            let mut lines: Vec<Line<'static>> = Vec::new();
            lines.push(Line::from(Span::styled(
                format!("Tool: {}", req.name),
                palette
                    .base_style()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                req.summary.clone(),
                palette.base_style().fg(palette.fg),
            )));
            lines.push(Line::from(""));
            for line in args_pretty.lines() {
                lines.push(Line::from(Span::styled(
                    line.to_string(),
                    palette.base_style().fg(palette.muted),
                )));
            }

            let body = Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .style(palette.base_style())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(palette.border).bg(palette.bg))
                        .padding(Padding::new(2, 2, 1, 0))
                        .title(Line::styled(
                            " Permission requested ",
                            palette.base_style().fg(palette.fg),
                        ))
                        .title_bottom(
                            Line::from(" [y] allow  [n] deny  [a] always  [N] never  [Esc] deny ")
                                .right_aligned(),
                        ),
                );
            frame.render_widget(body, popup);
        }
    }

    if let InputMode::BashNetworkPrompt { summary, .. } = &app.input_mode {
        let popup = centered_rect(60, 35, area);
        frame.render_widget(Clear, popup);

        let lines: Vec<Line<'static>> = vec![
            Line::from(Span::styled(
                "Bash needs network access".to_string(),
                palette
                    .base_style()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                summary.clone(),
                palette.base_style().fg(palette.fg),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  [O]nce              allow this invocation only".to_string(),
                palette.base_style().fg(palette.success),
            )),
            Line::from(Span::styled(
                "  [A]lways            allow for the rest of this session".to_string(),
                palette.base_style().fg(palette.success),
            )),
            Line::from(Span::styled(
                "  [D]eny once         deny this invocation only".to_string(),
                palette.base_style().fg(palette.error),
            )),
            Line::from(Span::styled(
                "  [F]orever (Never)   deny for the rest of this session".to_string(),
                palette.base_style().fg(palette.error),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Per-call override: re-run with `/bash --net <cmd>` or `/bash --no-net <cmd>`"
                    .to_string(),
                palette.base_style().fg(palette.muted),
            )),
        ];

        let body = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(palette.base_style())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(palette.border).bg(palette.bg))
                    .padding(Padding::new(2, 2, 1, 0))
                    .title(Line::styled(
                        " Bash network access? ",
                        palette.base_style().fg(palette.fg),
                    ))
                    .title_bottom(
                        Line::from(" [O]nce  [A]lways  [D]eny  [F]orever  [Esc] deny ")
                            .right_aligned(),
                    ),
            );
        frame.render_widget(body, popup);
    }

    if matches!(app.input_mode, InputMode::EnteringApiKey) {
        let popup = centered_rect(60, 20, area);
        frame.render_widget(Clear, popup);
        let title = match app.pending_provider {
            Some(spec) => format!(" {} API key ", spec.display_name),
            None => " API key ".to_string(),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette.border).bg(palette.bg))
            .style(palette.base_style())
            .title(Line::styled(title, palette.base_style().fg(palette.fg)))
            .title_bottom(Line::from(" [Enter] connect  [Esc] cancel ").right_aligned());
        let inner = popup.inner(Margin {
            horizontal: 2,
            vertical: 1,
        });
        frame.render_widget(block, popup);
        let mut ta = app.api_key_textarea.clone();
        ta.set_block(Block::default());
        ta.set_style(palette.base_style());
        frame.render_widget(&ta, inner);
    }

    if matches!(app.input_mode, InputMode::SelectingTranscript) {
        render_transcript_picker(app, frame, area, palette);
    }
}

fn render_transcript_picker(app: &App, frame: &mut Frame, area: Rect, palette: Palette) {
    let popup = centered_rect(70, 50, area);
    frame.render_widget(Clear, popup);

    if app.transcript_entries.is_empty() {
        let body = Paragraph::new("No transcripts found in ~/.otto/transcripts/")
            .style(palette.base_style())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(palette.border).bg(palette.bg))
                    .padding(Padding::new(2, 2, 1, 0))
                    .title(Line::styled(
                        " Resume transcript ",
                        palette.base_style().fg(palette.fg),
                    ))
                    .title_bottom(Line::from(" [Esc] cancel ").right_aligned()),
            );
        frame.render_widget(body, popup);
        return;
    }

    let items: Vec<ListItem> = app
        .transcript_entries
        .iter()
        .enumerate()
        .map(|(i, entry)| render_transcript_item(entry, i == app.transcript_index, palette))
        .collect();

    let list = List::new(items).style(palette.base_style()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette.border).bg(palette.bg))
            .padding(Padding::new(2, 2, 1, 0))
            .title(" Resume transcript ")
            .title_bottom(Line::from(" [↑/↓] move  [Enter] resume  [Esc] cancel ").right_aligned()),
    );
    frame.render_widget(list, popup);
}

fn render_transcript_item(
    entry: &TranscriptEntry,
    selected: bool,
    palette: Palette,
) -> ListItem<'static> {
    let style = if selected {
        palette
            .base_style()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        palette.base_style()
    };
    let meta_style = palette.base_style().fg(palette.muted);
    let line = Line::from(vec![
        Span::styled(format!("{:<22}", entry.timestamp), style),
        Span::styled(format!(" {:>3} msgs  ", entry.message_count), meta_style),
        Span::styled(entry.preview.clone(), palette.base_style().fg(palette.fg)),
    ]);
    ListItem::new(line)
}

/// One overlay region computed during the conversation-log render pass.
/// `id` keys into [`crate::app::CanvasRegistry`] for the source/renderer;
/// `area` is the on-screen rect (already clipped to `inner_area`) where
/// the image or fallback should paint. `streaming` is `true` when the
/// canvas is still receiving `HtmlSourceDelta`s — Task 17 turns this into
/// the live-source preview path; Task 16 reuses the same branch as the
/// "no image protocol" fallback so the rectangle is never blank.
#[derive(Debug, Clone)]
struct CanvasOverlay {
    id: ContentBlockId,
    area: Rect,
    streaming: bool,
    source: String,
}

fn render_log(
    app: &App,
    frame: &mut Frame,
    area: Rect,
    palette: Palette,
    frame_data: &HomeFrameData,
) -> Vec<CanvasOverlay> {
    /// Tracks each canvas's first line index in `lines` plus the metadata
    /// the overlay pass needs to draw it. We resolve the on-screen rect
    /// at the bottom of this function once `inner_area.width` and the
    /// scroll offset are known.
    struct CanvasMark {
        id: ContentBlockId,
        line_idx: usize,
        streaming: bool,
        source: String,
    }
    let mut canvas_marks: Vec<CanvasMark> = Vec::new();
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(app.entries.len() * 2 + 1);
    let mut tool_entry_idx: usize = 0;
    for entry in &app.entries {
        match entry {
            Entry::User(text) => {
                lines.push(line_block(
                    rust_i18n::t!("conversation.you-prefix").as_ref(),
                    text,
                    palette.success,
                    palette,
                ));
            }
            Entry::Assistant(text) => {
                lines.push(line_block(
                    rust_i18n::t!("conversation.agent-prefix").as_ref(),
                    text,
                    palette.secondary,
                    palette,
                ));
            }
            Entry::Tool {
                name: _,
                args: _,
                status,
                result_text: _,
            } => {
                debug_assert!(
                    tool_entry_idx < frame_data.tool_entries.len(),
                    "tool_entries index out of bounds — stale frame data (idx={tool_entry_idx}, len={})",
                    frame_data.tool_entries.len()
                );
                let render = frame_data
                    .tool_entries
                    .get(tool_entry_idx)
                    .cloned()
                    .unwrap_or(ToolEntryRender {
                        arg_spans: vec![],
                        result_spans: None,
                    });
                tool_entry_idx += 1;

                let badge = match status {
                    None => "…",
                    Some(ToolCallStatus::Ok) => "✓",
                    Some(ToolCallStatus::Errored) => "✗",
                };
                let badge_color = match status {
                    None => palette.warning,
                    Some(ToolCallStatus::Ok) => palette.success,
                    Some(ToolCallStatus::Errored) => palette.error,
                };

                // Arguments line: badge prefix + pre-rendered styled spans.
                let mut arg_line_spans: Vec<Span<'static>> = vec![Span::styled(
                    format!("  {badge} "),
                    palette.base_style().fg(badge_color),
                )];
                for s in render.arg_spans {
                    arg_line_spans
                        .push(crate::plugin::convert::styled_span_to_ratatui(s, &palette));
                }
                lines.push(Line::from(arg_line_spans));

                // Result line (if any): pre-rendered styled spans, indented.
                if let Some(result_spans) = render.result_spans {
                    let mut result_line_spans: Vec<Span<'static>> = vec![Span::styled(
                        "    → ".to_string(),
                        palette.base_style().fg(palette.muted),
                    )];
                    for s in result_spans {
                        result_line_spans
                            .push(crate::plugin::convert::styled_span_to_ratatui(s, &palette));
                    }
                    lines.push(Line::from(result_line_spans));
                }
            }
            Entry::RouteBadge(text) => {
                // Muted single line. Style matches Entry::Note; the leading
                // glyph distinguishes routing decisions from generic notices.
                lines.push(Line::from(Span::styled(
                    format!("▸ {text}"),
                    palette
                        .base_style()
                        .fg(palette.muted)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
            Entry::Note(text) => {
                lines.push(Line::from(Span::styled(
                    format!("· {text}"),
                    palette
                        .base_style()
                        .fg(palette.muted)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
            // Canvas entries reserve a fixed block of blank rows in the
            // paragraph so the dedicated overlay pass (see
            // `render_canvas_overlays`) has space to draw the image or
            // source-code fallback at the right vertical position. The
            // first reserved row carries a one-line caption that stays
            // visible even when overlay rendering is unavailable (e.g.,
            // mid-frame during a terminal resize).
            Entry::Canvas {
                id,
                source,
                source_preview,
            } => {
                let caption = if source_preview.is_some() {
                    format!("⬛ canvas:{} — streaming source…", id.0)
                } else {
                    format!("⬛ canvas:{}", id.0)
                };
                let start_idx = lines.len();
                lines.push(Line::from(Span::styled(
                    caption,
                    palette.base_style().fg(palette.muted),
                )));
                // CANVAS_RESERVED_ROWS - 1 blank rows below the caption.
                for _ in 1..CANVAS_RESERVED_ROWS {
                    lines.push(Line::from(""));
                }
                canvas_marks.push(CanvasMark {
                    id: *id,
                    line_idx: start_idx,
                    streaming: source_preview.is_some(),
                    source: source_preview.clone().unwrap_or_else(|| source.clone()),
                });
            }
        }
    }

    if !app.live_text.is_empty() {
        lines.push(line_block(
            rust_i18n::t!("conversation.agent-prefix").as_ref(),
            &app.live_text,
            palette.secondary,
            palette,
        ));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.border).bg(palette.bg))
        .padding(Padding::new(2, 2, 1, 1))
        .title(Line::styled(
            " Conversation ",
            palette.base_style().fg(palette.fg),
        ));
    // `inner_area` excludes the border + padding, so `line_count(width)` and
    // `area.height` agree on the same coordinate space.
    let inner_area = block.inner(area);

    // Pre-compute the canvas overlay positions BEFORE moving `lines` into
    // `Paragraph::new`. We need each placeholder's wrapped-row offset from
    // the top of the paragraph, and the simple way to get that is to walk
    // the line vector once at the same `inner_area.width` the paragraph
    // will wrap at.
    let inner_width = inner_area.width as usize;
    let line_rows: Vec<usize> = lines
        .iter()
        .map(|line| wrapped_row_count(line.width(), inner_width))
        .collect();
    let mut cum: Vec<usize> = Vec::with_capacity(line_rows.len());
    let mut running = 0usize;
    for n in &line_rows {
        cum.push(running);
        running += *n;
    }
    let total_wrapped_rows = running;

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .style(palette.base_style());

    // Auto-tail by default: scroll so the LAST wrapped line lands on the
    // bottom row of `inner_area`. Ratatui's Paragraph renders top-down, so
    // without a scroll offset newly-streamed text falls off the bottom and
    // becomes invisible. See `log_scroll_y` for the cascade.
    //
    // We use our own `total_wrapped_rows` rather than
    // `para.line_count(inner_area.width)` so the auto-tail and the canvas
    // overlay placement below agree on the same row coordinate system.
    // Ratatui's `Wrap { trim: false }` word-wraps at whitespace; our
    // `wrapped_row_count` divides by cell width. The two can disagree by
    // a few rows when very long Assistant lines word-wrap. Keeping both
    // sides on the same approximation avoids a visual mismatch between
    // where the canvas placeholder paints and where the overlay draws —
    // at the cost of slightly imprecise auto-tail for pathologically
    // long unwrapped lines.
    let scroll_y = log_scroll_y(
        total_wrapped_rows,
        inner_area.height as usize,
        app.log_scroll_offset_from_bottom,
    );

    frame.render_widget(para.scroll((scroll_y, 0)).block(block), area);

    // Resolve each canvas placeholder's on-screen rect using the cumulative
    // wrapped-row offsets computed above. Emit a `CanvasOverlay` for every
    // placeholder whose top row lands in the visible band
    // `[scroll_y, scroll_y + inner_area.height)`. Rects are clipped to
    // `inner_area` so the overlay never bleeds into the bordered block's
    // chrome.
    if canvas_marks.is_empty() || inner_area.width == 0 || inner_area.height == 0 {
        return Vec::new();
    }

    let viewport_top = scroll_y as usize;
    let viewport_bottom = viewport_top.saturating_add(inner_area.height as usize);

    let mut overlays: Vec<CanvasOverlay> = Vec::with_capacity(canvas_marks.len());
    for mark in canvas_marks {
        let placeholder_top = cum.get(mark.line_idx).copied().unwrap_or(0);
        // Each placeholder occupies CANVAS_RESERVED_ROWS contiguous blank
        // rows (1-row caption + N-1 blank rows), each of which wraps to
        // exactly one screen row because `Line::from("")` has width 0.
        let placeholder_bottom = placeholder_top + CANVAS_RESERVED_ROWS as usize;

        // Skip canvases entirely outside the viewport.
        if placeholder_bottom <= viewport_top || placeholder_top >= viewport_bottom {
            continue;
        }

        // Clip the placeholder rect to the visible band.
        let visible_top = placeholder_top.max(viewport_top);
        let visible_bottom = placeholder_bottom.min(viewport_bottom);
        let on_screen_y = (visible_top - viewport_top) as u16;
        let height = (visible_bottom - visible_top) as u16;
        if height == 0 {
            continue;
        }

        let rect = Rect {
            x: inner_area.x,
            y: inner_area.y + on_screen_y,
            width: inner_area.width,
            height,
        };

        overlays.push(CanvasOverlay {
            id: mark.id,
            area: rect,
            streaming: mark.streaming,
            source: mark.source,
        });
    }
    overlays
}

/// Wrapped-row count for a single logical line of visual width `w` at
/// `wrap_width`. Empty lines and 0-width wrap_widths collapse to 1 row.
fn wrapped_row_count(w: usize, wrap_width: usize) -> usize {
    if wrap_width == 0 {
        return 1;
    }
    if w == 0 {
        return 1;
    }
    w.div_ceil(wrap_width)
}

/// Paint each `Entry::Canvas` overlay over the conversation log. Three
/// branches:
///
/// 1. **Streaming.** `overlay.streaming` is `true` — the host is still
///    accumulating `HtmlSourceDelta`s. Render the partial source as a
///    code block with a "rendering…" hint. Phase 1 reuses the same path
///    as the no-image-protocol fallback so something always paints.
/// 2. **Image protocol available, complete source.** Drive the renderer,
///    convert the resulting `Frame` to a `StatefulProtocol`, and hand
///    `StatefulImage` to `render_stateful_widget`. The protocol is cached
///    on `CanvasRegistry::image_states`; the stateful widget re-encodes
///    internally when the area changes.
/// 3. **No image protocol.** Same code-block fallback as (1), with a
///    one-line yellow banner explaining the situation.
///
/// `overlay.area` is already clipped to the conversation block's inner
/// area, so any of the above paint inside the bordered "Conversation" box
/// without bleeding into the chrome.
fn render_canvas_overlays(
    app: &mut App,
    frame: &mut Frame,
    palette: Palette,
    overlays: &[CanvasOverlay],
) {
    for overlay in overlays {
        if overlay.area.width == 0 || overlay.area.height == 0 {
            continue;
        }
        // Focus chrome: when this canvas is the focused element, paint a
        // 1-cell accent border around the overlay and render the content
        // into the block's inner area. Unfocused canvases render as before.
        let focused = app.is_canvas_focused(overlay.id);
        let content_area = canvas_content_area(overlay.area, focused);
        if focused {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Plain)
                .border_style(palette.base_style().fg(palette.accent));
            frame.render_widget(block, overlay.area);
        }
        // A 1-cell shrink on a thin overlay can collapse the inner area to
        // zero — skip content rendering then (matches Phase 1's small-area
        // behavior), leaving just the border.
        if content_area.width == 0 || content_area.height == 0 {
            continue;
        }
        if overlay.streaming {
            render_source_preview(frame, content_area, &overlay.source, palette);
            continue;
        }
        if app.canvas_registry.image_protocol_available() {
            render_canvas_image(
                frame,
                content_area,
                app,
                overlay.id,
                palette,
                &overlay.source,
            );
        } else {
            render_canvas_source_fallback(frame, content_area, &overlay.source, palette);
        }
    }
}

/// Inner area available for canvas content given the overlay rect and focus
/// state. A focused canvas reserves 1 cell on every side for its accent
/// border (via `Block::inner`); an unfocused canvas uses the full rect. The
/// shrink is saturating, so a tiny focused overlay collapses to a zero-size
/// area rather than panicking — callers must guard against that.
fn canvas_content_area(area: Rect, focused: bool) -> Rect {
    if !focused {
        return area;
    }
    Block::default().borders(Borders::ALL).inner(area)
}

/// Drive the canvas renderer at `area`'s pixel width and overlay the
/// resulting image. Falls back to the source-code path on any failure
/// (no renderer registered, malformed frame, etc.) so the user still
/// sees the HTML they asked the model for.
fn render_canvas_image(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    id: ContentBlockId,
    palette: Palette,
    source: &str,
) {
    // Pixel width = cell width * picker-reported font width.
    let cell_w = match app.canvas_registry.image_cell_size() {
        Some((w, _h)) => w,
        None => {
            // Defensive: image_protocol_available() returned true above,
            // but threads-of-control change could in theory flip it.
            render_canvas_source_fallback(frame, area, source, palette);
            return;
        }
    };
    let pixel_width = (area.width as u32).saturating_mul(cell_w as u32);
    if pixel_width == 0 {
        render_canvas_source_fallback(frame, area, source, palette);
        return;
    }

    // Clear the overlay rect so any stale text (placeholder caption or
    // adjacent paragraph) doesn't bleed through the rendered image.
    frame.render_widget(Clear, area);
    frame.buffer_mut().set_style(area, palette.base_style());

    let protocol = app.canvas_registry.image_protocol_mut(id, pixel_width);
    match protocol {
        Some(state) => {
            let widget =
                ratatui_image::StatefulImage::<ratatui_image::protocol::StatefulProtocol>::default(
                );
            frame.render_stateful_widget(widget, area, state);
        }
        None => {
            // Renderer missing or frame empty/mis-sized — show the source
            // so the user still sees what the model emitted.
            render_canvas_source_fallback(frame, area, source, palette);
        }
    }
}

/// Source-code fallback used when the terminal lacks an image protocol.
/// Top row is a yellow banner naming the supported terminals; the rest of
/// the area renders the HTML source inside a left-bar "code block".
fn render_canvas_source_fallback(frame: &mut Frame, area: Rect, source: &str, palette: Palette) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    frame.render_widget(Clear, area);
    frame.buffer_mut().set_style(area, palette.base_style());

    let banner = Paragraph::new(
        "Inline HTML rendering requires kitty / WezTerm / Ghostty / iTerm2 / Sixel.",
    )
    .style(palette.base_style().fg(palette.warning))
    .wrap(Wrap { trim: false });
    let (banner_area, body_area) = split_top_one_line(area);
    frame.render_widget(banner, banner_area);
    if body_area.height > 0 {
        render_code_block(frame, body_area, source, palette);
    }
}

/// Streaming preview: same code-block helper plus a muted "rendering…"
/// header. Task 17 will swap this for a syntax-highlighted variant; the
/// header keeps the user-facing affordance stable.
fn render_source_preview(frame: &mut Frame, area: Rect, preview: &str, palette: Palette) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    frame.render_widget(Clear, area);
    frame.buffer_mut().set_style(area, palette.base_style());

    let header =
        Paragraph::new("Rendering HTML canvas…").style(palette.base_style().fg(palette.muted));
    let (header_area, body_area) = split_top_one_line(area);
    frame.render_widget(header, header_area);
    if body_area.height > 0 {
        render_code_block(frame, body_area, preview, palette);
    }
}

/// Minimal "code block" widget: muted text inside a left-border bar.
/// Phase 2 will plumb syntect-driven syntax highlighting through; Phase 1
/// keeps the surface flat to avoid dragging in another renderer dep.
fn render_code_block(frame: &mut Frame, area: Rect, source: &str, palette: Palette) {
    let widget = Paragraph::new(source.to_string())
        .wrap(Wrap { trim: false })
        .style(palette.base_style().fg(palette.fg))
        .block(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(palette.base_style().fg(palette.border)),
        );
    frame.render_widget(widget, area);
}

/// Split `area` into a 1-row top strip and the remainder. When `area` is
/// 1 row tall the top strip takes the whole rect and the body has height 0.
fn split_top_one_line(area: Rect) -> (Rect, Rect) {
    if area.height == 0 {
        return (area, area);
    }
    let top = Rect {
        height: 1.min(area.height),
        ..area
    };
    let body = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };
    (top, body)
}

fn line_block(prefix: &str, text: &str, color: Color, palette: Palette) -> Line<'static> {
    let style = palette.base_style().fg(color);
    Line::from(vec![
        Span::styled(prefix.to_string(), style.add_modifier(Modifier::BOLD)),
        Span::styled(text.to_string(), style),
    ])
}

/// The prompt textarea's own block: bordered, background/border colours
/// from the active theme, horizontal padding. Factored out of `render()` so
/// the ghost-completion overlay (see [`paint_ghost_completion`]) can derive
/// its interior rect from the exact same `Block` value the textarea itself
/// renders with, rather than a second, independently-written literal that
/// could silently drift out of sync with this one.
fn prompt_block(palette: Palette) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette.border).bg(palette.bg))
        .padding(Padding::horizontal(1))
}

/// Paints `ghost` as dim text immediately after the prompt's cursor, if
/// doing so is safe. Pure function over a `Buffer` and primitives (no
/// `Frame`/`App` dependency) so it is directly unit-testable — `render()`
/// itself has no test precedent in this file and pulling this logic out
/// avoids inventing one.
///
/// Safety conditions, all required:
/// - `inner` is clamped to `buf`'s own area before anything else runs. A
///   degenerate or off-buffer `inner` (e.g. a squeezed layout on a very
///   short terminal, where `Block::inner` can clamp `y` to the frame's
///   bottom edge) would otherwise reach `Buffer::set_stringn`, whose
///   underlying cell write panics on an out-of-area index — not gated
///   behind `debug_assertions`. Clamping turns that into a safe no-op.
///   (Caught in PR review for issue #118: a reachable panic on short
///   terminals, e.g. mid-resize while the palette is open.)
/// - `cursor.0 == 0` and `lines.len() == 1`: the cursor is on the
///   textarea's one and only logical line.
/// - `line_fits`: that line's full character count is within the clamped
///   `inner`'s width. This is the actual no-wrap proof — `WrapMode::WordOrGlyph`
///   cannot have moved the cursor's visual column away from its logical
///   column unless the line was wider than the available width, so
///   checking `cursor.0 == 0 && lines.len() == 1` alone is NOT sufficient:
///   both report *logical* line/row and are unchanged by soft-wrapping a
///   long logical line across multiple *visual* rows. (Caught in design
///   review for issue #118 — see that spec's Risks section.)
fn paint_ghost_completion(
    buf: &mut Buffer,
    inner: Rect,
    cursor: (usize, usize),
    lines: &[String],
    ghost: &str,
    style: Style,
) {
    let inner = inner.intersection(buf.area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let (cursor_row, cursor_col) = cursor;
    let single_line = lines.len() == 1;
    let line_fits = lines
        .first()
        .is_some_and(|l| l.chars().count() <= inner.width as usize);
    if cursor_row != 0 || !single_line || !line_fits {
        return;
    }
    let x = inner.x.saturating_add(cursor_col as u16);
    if x >= inner.x + inner.width {
        return;
    }
    let max_width = (inner.x + inner.width - x) as usize;
    buf.set_stringn(x, inner.y, ghost, max_width, style);
}

/// Place a `BottomSheet` of `height` rows inside `area`, anchored so its
/// bottom edge meets the top of the prompt textarea (`input_top`) rather
/// than the bottom of the whole frame — the inline `/`-palette look, with
/// the input row left visible immediately below the sheet.
///
/// The height is clamped to the space that actually exists *above* the
/// prompt. Without that clamp a short terminal (the home layout's minimum
/// is header 3 + log 1 + banner 1 + tips 1 + input 3 + footer 1, so a
/// 15-row terminal puts `input_top` at 11) would pin the sheet's top at
/// `area.y` while keeping its full height, growing it back down over the
/// header and the textarea — exactly what the anchoring exists to avoid.
fn bottom_sheet_rect(area: Rect, input_top: u16, height: u16) -> Rect {
    let bottom = input_top.max(area.y);
    let h = height.min(bottom.saturating_sub(area.y)).min(area.height);
    Rect::new(area.x, bottom.saturating_sub(h), area.width, h)
}

/// Paint a plugin-provided screen over `area`, using the screen's declared
/// [`otto_plugin::ScreenLayout`] to position it.
///
/// For `CenteredModal`, the host draws the border and title so the
/// screen's `render` output fills the inner content area.
/// For `Fullscreen`, content fills the computed area directly.
/// For `BottomSheet`, content is anchored directly above the prompt
/// textarea (`input_top`) rather than the bottom of the whole terminal —
/// this is the inline `/`-command-palette-style overlay: the input row
/// stays visible immediately below the sheet instead of being covered.
///
/// Every layout punches a hole with [`Clear`] and then fills its region
/// with `palette.base_style()` so the modal sits on a uniform theme
/// background. Without that step the conversation log behind the modal
/// would bleed through under any plugin span that only sets `fg` — which
/// makes upstream themes (Solarized Light, Catppuccin Latte, Tokyo Night
/// Day, …) look like floating text rather than a popup.
struct ActiveScreen<'a> {
    id: &'a str,
    screen: &'a dyn otto_plugin::Screen,
    layout: &'a otto_plugin::ScreenLayout,
}

fn paint_screen(
    f: &mut Frame,
    area: Rect,
    input_top: u16,
    active_screen: ActiveScreen<'_>,
    palette: Palette,
    splash_sandbox: &crate::splash::SandboxSplashState,
) {
    use otto_plugin::ScreenLayout;

    match active_screen.layout {
        ScreenLayout::Fullscreen { .. } => {
            if active_screen.id == "splash" {
                crate::splash::render(f, area, splash_sandbox);
                return;
            }
            // Full-frame overlay: paint content directly.
            f.render_widget(Clear, area);
            f.buffer_mut().set_style(area, palette.base_style());

            // Reserve the bottom row for tips() before handing the region to
            // the screen, so its content never lands under the tips line.
            let tips = active_screen.screen.tips();
            let content_area = if !tips.is_empty() && area.height > 0 {
                Rect::new(area.x, area.y, area.width, area.height - 1)
            } else {
                area
            };
            let region = crate::plugin::convert::rect_to_region(content_area);
            let lines: Vec<Line<'static>> = active_screen
                .screen
                .render(region)
                .into_iter()
                .map(|l| crate::plugin::convert::styled_line_to_ratatui(l, &palette))
                .collect();
            let para = Paragraph::new(lines).style(palette.base_style());
            f.render_widget(para, content_area);

            // Tips row at the very bottom of the frame.
            if !tips.is_empty() && area.height > 0 {
                let tips_row = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
                let tips_lines: Vec<Line<'static>> = tips
                    .into_iter()
                    .map(|l| crate::plugin::convert::styled_line_to_ratatui(l, &palette))
                    .collect();
                f.render_widget(
                    Paragraph::new(tips_lines).style(palette.base_style()),
                    tips_row,
                );
            }
        }
        ScreenLayout::CenteredModal {
            width_pct,
            height_pct,
            title,
        } => {
            // Compute the outer rect for the modal border.
            let w = ((area.width as u32 * (*width_pct as u32)) / 100)
                .max(20)
                .min(area.width as u32) as u16;
            let h = ((area.height as u32 * (*height_pct as u32)) / 100)
                .max(5)
                .min(area.height as u32) as u16;
            let x = area.x + area.width.saturating_sub(w) / 2;
            let y = area.y + area.height.saturating_sub(h) / 2;
            let outer = Rect::new(x, y, w, h);

            // Punch a hole over whatever's underneath, then fill the modal's
            // region with the theme's base style so spans that only set fg
            // sit on a uniform bg instead of the conversation log behind.
            f.render_widget(Clear, outer);
            f.buffer_mut().set_style(outer, palette.base_style());

            // Border + optional title.
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(palette.border).bg(palette.bg))
                .style(palette.base_style())
                .title(Line::styled(
                    title.as_deref().unwrap_or("").to_string(),
                    palette.base_style().fg(palette.fg),
                ));

            // Tips as a bottom title if present.
            let tips = active_screen.screen.tips();
            let block = if let Some(tip_line) = tips.into_iter().next() {
                let tip_text: String = tip_line.spans.iter().map(|s| s.text.as_str()).collect();
                block.title_bottom(Line::from(tip_text).right_aligned())
            } else {
                block
            };

            // Interior padding: 2 cols horizontally and 1 row top/bottom
            // gives modal content breathing room inside the border.
            let inner = outer.inner(Margin {
                horizontal: 2,
                vertical: 1,
            });
            f.render_widget(block, outer);

            let region = crate::plugin::convert::rect_to_region(inner);
            let lines: Vec<Line<'static>> = active_screen
                .screen
                .render(region)
                .into_iter()
                .map(|l| crate::plugin::convert::styled_line_to_ratatui(l, &palette))
                .collect();
            f.render_widget(Paragraph::new(lines).style(palette.base_style()), inner);
        }
        ScreenLayout::BottomSheet { height } => {
            let sheet = bottom_sheet_rect(area, input_top, *height);
            f.render_widget(Clear, sheet);
            f.buffer_mut().set_style(sheet, palette.base_style());

            // Reserve the bottom row for tips() before handing the region to
            // the screen, so its content never lands under the tips line.
            let tips = active_screen.screen.tips();
            let content_sheet = if !tips.is_empty() && sheet.height > 0 {
                Rect::new(sheet.x, sheet.y, sheet.width, sheet.height - 1)
            } else {
                sheet
            };
            let region = crate::plugin::convert::rect_to_region(content_sheet);
            let lines: Vec<Line<'static>> = active_screen
                .screen
                .render(region)
                .into_iter()
                .map(|l| crate::plugin::convert::styled_line_to_ratatui(l, &palette))
                .collect();
            f.render_widget(
                Paragraph::new(lines).style(palette.base_style()),
                content_sheet,
            );

            if !tips.is_empty() && sheet.height > 0 {
                let tips_row = Rect::new(sheet.x, sheet.y + sheet.height - 1, sheet.width, 1);
                let tips_lines: Vec<Line<'static>> = tips
                    .into_iter()
                    .map(|l| crate::plugin::convert::styled_line_to_ratatui(l, &palette))
                    .collect();
                f.render_widget(
                    Paragraph::new(tips_lines).style(palette.base_style()),
                    tips_row,
                );
            }
        }
        // Future layout variants are silently treated as fullscreen.
        _ => {
            f.render_widget(Clear, area);
            f.buffer_mut().set_style(area, palette.base_style());
            let region = crate::plugin::convert::rect_to_region(area);
            let lines: Vec<Line<'static>> = active_screen
                .screen
                .render(region)
                .into_iter()
                .map(|l| crate::plugin::convert::styled_line_to_ratatui(l, &palette))
                .collect();
            f.render_widget(Paragraph::new(lines).style(palette.base_style()), area);
        }
    }
}

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn compose_footer_ratatui_line<const N: usize>(
    groups: [&[Line<'static>]; N],
    separator: &Span<'static>,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for group in groups {
        for line in group {
            if line.spans.is_empty() {
                continue;
            }
            if !spans.is_empty() {
                spans.push(separator.clone());
            }
            spans.extend(line.spans.iter().cloned());
        }
    }
    Line::from(spans)
}

fn footer_center_lines(
    center: &[otto_plugin::StyledLine],
    turn_line_idx: Option<usize>,
    active_turn_id: Option<u32>,
    pending_turn_id: Option<u32>,
    tick: u64,
    palette: Palette,
) -> Vec<Line<'static>> {
    center
        .iter()
        .cloned()
        .enumerate()
        .map(|(idx, line)| {
            let busy_turn_id = active_turn_id.or(pending_turn_id);
            let rewrite_for_pending =
                active_turn_id.is_none() && pending_turn_id.is_some() && Some(idx) == turn_line_idx;
            let use_busy_line =
                busy_turn_id.is_some() && Some(idx) == turn_line_idx && !line.spans.is_empty();
            let mut line = if rewrite_for_pending && !line.spans.is_empty() {
                crate::plugin::convert::styled_line_to_ratatui(
                    otto_plugin::StyledLine::colored(
                        rust_i18n::t!(
                            "footer.turn-working",
                            id = pending_turn_id.expect("checked is_some above")
                        )
                        .to_string(),
                        otto_plugin::ThemeColor::Accent,
                    ),
                    &palette,
                )
            } else {
                crate::plugin::convert::styled_line_to_ratatui(line, &palette)
            };
            if !use_busy_line || line.spans.is_empty() {
                return line;
            }

            let spinner = footer_spinner_spans(tick, palette);
            if spinner.is_empty() {
                return line;
            }
            line.spans.push(Span::raw(" "));
            line.spans.extend(spinner);
            line
        })
        .collect()
}

fn footer_spinner_spans(tick: u64, palette: Palette) -> Vec<Span<'static>> {
    let area = Rect::new(0, 0, 2, 1);
    let mut buffer = Buffer::empty(area);
    CircleSpinner::new(tick)
        .radius(1)
        .arc_color(palette.accent)
        .dim_color(palette.muted)
        .render(area, &mut buffer);

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_text = String::new();
    let mut current_style: Option<Style> = None;

    for cell in buffer.content().iter().take(area.width as usize) {
        let symbol = cell.symbol();
        if symbol == " " {
            continue;
        }

        let style = cell.style();
        if current_style.as_ref() == Some(&style) {
            current_text.push_str(symbol);
            continue;
        }

        if let Some(style) = current_style.replace(style) {
            spans.push(Span::styled(std::mem::take(&mut current_text), style));
        }
        current_text.push_str(symbol);
    }

    if let Some(style) = current_style {
        spans.push(Span::styled(current_text, style));
    }

    spans
}

fn provider_selector_shows_query(provider_count: usize, provider_query: &str) -> bool {
    !provider_query.is_empty() || provider_count > PROVIDER_SELECTOR_DISCOVERABILITY_THRESHOLD
}

#[derive(Debug, Clone)]
struct ProviderSelectorItem {
    spec: &'static ProviderSpec,
    is_selected: bool,
    is_active: bool,
}

#[derive(Debug, Clone)]
struct ProviderSelectorView {
    show_query_row: bool,
    query_text: Option<String>,
    query_is_placeholder: bool,
    items: Vec<ProviderSelectorItem>,
    empty_state: Option<String>,
    help_text: &'static str,
}

fn build_provider_selector_view(app: &App, provider_count: usize) -> ProviderSelectorView {
    let show_query_row = provider_selector_shows_query(provider_count, app.provider_query.as_str());
    let items: Vec<ProviderSelectorItem> = app
        .filtered_providers()
        .iter()
        .enumerate()
        .map(|(i, spec)| ProviderSelectorItem {
            spec,
            is_selected: i == app.provider_index,
            is_active: Some(spec.id) == app.active_provider_id,
        })
        .collect();
    let empty_state = items.is_empty().then(|| {
        format!(
            "No providers match `{}`. Keep typing or press Esc to clear.",
            app.provider_query
        )
    });
    let (query_text, query_is_placeholder) = if show_query_row {
        if app.provider_query.is_empty() {
            (Some("type to filter".to_string()), true)
        } else {
            (Some(app.provider_query.clone()), false)
        }
    } else {
        (None, false)
    };

    ProviderSelectorView {
        show_query_row,
        query_text,
        query_is_placeholder,
        items,
        empty_state,
        help_text: " [type] filter  [↑/↓] move  [Enter] select  [Esc] clear/cancel ",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::plugin::builtin::themes::catalog::Theme;
    use async_trait::async_trait;
    use otto_plugin::{
        Effect, KeyEventPortable, PluginError, Region, Screen, ScreenLayout, StyledLine, ThemeColor,
    };
    use ratatui::{Terminal, backend::TestBackend};
    use std::path::PathBuf;

    fn rline(text: &str) -> Line<'static> {
        Line::from(vec![Span::raw(text.to_string())])
    }

    fn rsep() -> Span<'static> {
        Span::styled(" · ".to_string(), Style::default().fg(palette().muted))
    }

    fn palette() -> Palette {
        Palette::for_theme(Theme::Dark)
    }

    fn locale_lock() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::test_helpers::HOME_LOCK.lock().unwrap();
        rust_i18n::set_locale("en");
        guard
    }

    fn joined_ratatui(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn fresh_app() -> App {
        App::new(String::new(), PathBuf::from("."), "en".to_string())
    }

    struct FakeScreen {
        id: String,
        body: Vec<StyledLine>,
        tips: Vec<StyledLine>,
    }

    #[async_trait]
    impl Screen for FakeScreen {
        fn id(&self) -> String {
            self.id.clone()
        }

        fn render(&self, _region: Region) -> Vec<StyledLine> {
            self.body.clone()
        }

        async fn on_key(&mut self, _key: KeyEventPortable) -> Result<Vec<Effect>, PluginError> {
            Ok(vec![])
        }

        fn tips(&self) -> Vec<StyledLine> {
            self.tips.clone()
        }
    }

    fn render_paint_screen(
        screen: &dyn Screen,
        layout: &ScreenLayout,
        palette: Palette,
        splash_sandbox: crate::splash::SandboxSplashState,
    ) -> Buffer {
        let screen_id = screen.id();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                paint_screen(
                    frame,
                    area,
                    area.bottom(),
                    ActiveScreen {
                        id: &screen_id,
                        screen,
                        layout,
                    },
                    palette,
                    &splash_sandbox,
                );
            })
            .expect("paint_screen draw succeeds");
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buffer: &Buffer) -> String {
        let area = buffer.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    /// Paint a real `PaletteScreen` through the real `paint_screen` and
    /// look at the resulting cells. The palette's own unit tests check the
    /// `Vec<StyledLine>` it emits; this checks where those lines actually
    /// land, which is what the reserved-row arithmetic in `render` is for:
    /// the host overpaints the sheet's last row with `tips()` *after* the
    /// paragraph, so the selected row has to stay above it.
    ///
    /// Scope: this paints the sheet only. The prompt textarea is painted
    /// by `ui()`, not `paint_screen`, so it is not in this buffer — the
    /// absence of the `> <filter>` header is asserted here, but that the
    /// filter still reaches the prompt is covered by the palette's own
    /// `prompt_preview` tests, not by this one.
    #[tokio::test]
    async fn palette_sheet_paints_no_filter_header() {
        use crate::plugin::builtin::command_palette::screen::{PaletteCommand, PaletteScreen};
        use otto_plugin::KeyCodePortable;

        let cmd = |name: &str| PaletteCommand {
            name: name.into(),
            description: format!("{name} description"),
            needs_arg: false,
        };
        let mut screen = PaletteScreen::with_commands(vec![
            cmd("changelog"),
            cmd("clear"),
            cmd("connect"),
            cmd("exit"),
        ]);
        // Type `c` — three commands match, and under the old behavior the
        // sheet would have drawn `> c` directly above the prompt.
        screen
            .on_key(KeyEventPortable {
                code: KeyCodePortable::Char('c'),
                modifiers: otto_plugin::KeyMods::default(),
            })
            .await
            .expect("palette handles a char key");

        let buffer = render_paint_screen(
            &screen,
            &ScreenLayout::BottomSheet { height: 12 },
            Palette::for_theme(Theme::Dark),
            crate::splash::SandboxSplashState::OnDefault,
        );
        let text = buffer_text(&buffer);

        assert!(
            !text.contains("> c"),
            "the sheet must not draw its own filter header:\n{text}"
        );
        for name in ["/changelog", "/clear", "/connect"] {
            assert!(text.contains(name), "expected {name} in:\n{text}");
        }
        assert!(
            !text.contains("/exit"),
            "a filtered-out command must not paint:\n{text}"
        );
        assert!(
            text.contains("\u{25b6} /changelog"),
            "the highlighted row must paint its marker:\n{text}"
        );
    }

    /// The painted counterpart to `screen.rs`'s height sweep: with a list
    /// long enough to overflow the sheet and the cursor walked to the end,
    /// the window anchors the selected row on its *last* row — which is
    /// exactly the row the host would overpaint with `tips()` if the
    /// reserved-row arithmetic were off by one.
    ///
    /// The short-list case cannot test this: the marker lands on the
    /// sheet's second row for any value of the constant.
    #[tokio::test]
    async fn palette_selected_row_paints_above_the_tips_row_when_scrolled() {
        use crate::plugin::builtin::command_palette::screen::{PaletteCommand, PaletteScreen};
        use otto_plugin::KeyCodePortable;

        let mut screen = PaletteScreen::with_commands(
            (0..30)
                .map(|i| PaletteCommand {
                    name: format!("cmd{i:02}"),
                    description: format!("cmd{i:02} description"),
                    needs_arg: false,
                })
                .collect(),
        );
        for _ in 0..29 {
            screen
                .on_key(KeyEventPortable {
                    code: KeyCodePortable::Down,
                    modifiers: otto_plugin::KeyMods::default(),
                })
                .await
                .expect("palette handles Down");
        }

        // Derive the tips needle from the screen itself: rust_i18n's locale
        // is process-global and other tests in this binary switch it, so a
        // hardcoded English string would be a latent flake.
        let tips_text: String = screen.tips()[0]
            .spans
            .iter()
            .map(|s| s.text.clone())
            .collect();
        let tips_needle = tips_text
            .split(' ')
            .next()
            .expect("tips line is non-empty")
            .to_string();

        let buffer = render_paint_screen(
            &screen,
            &ScreenLayout::BottomSheet { height: 12 },
            Palette::for_theme(Theme::Dark),
            crate::splash::SandboxSplashState::OnDefault,
        );
        let text = buffer_text(&buffer);

        let marker_row = text
            .lines()
            .position(|l| l.contains('\u{25b6}'))
            .unwrap_or_else(|| panic!("the selected row must paint:\n{text}"));
        let tips_row = text
            .lines()
            .position(|l| l.contains(&tips_needle))
            .unwrap_or_else(|| panic!("the tips row must paint:\n{text}"));
        assert!(
            marker_row < tips_row,
            "selected row {marker_row} must paint above the tips row {tips_row}:\n{text}"
        );
        assert!(
            text.contains("/cmd29"),
            "the row the cursor is on must be visible:\n{text}"
        );
    }

    /// The `changelog` screen's own regression for the tips-overpaint contract:
    /// paint the real `ChangelogScreen` (not a synthetic stand-in) through the
    /// real `paint_screen`, in its actual `CenteredModal` layout, and confirm
    /// its last visible content row survives alongside the tips row rather
    /// than being hidden by it. `changelog` uses `CenteredModal`, where
    /// `tips()` paints as the modal border's bottom title
    /// (`paint_screen`'s `CenteredModal` arm) rather than by shrinking the
    /// content region — a structurally different mechanism than the
    /// `Fullscreen`/`BottomSheet` reservation the command-palette tests cover,
    /// so it needs its own concrete-screen proof. Mirrors
    /// `palette_selected_row_paints_above_the_tips_row_when_scrolled`.
    #[test]
    fn changelog_last_content_row_survives_its_own_tips_row() {
        use crate::plugin::builtin::changelog::screen::{ChangelogScreen, ChangelogState};
        use std::sync::{Arc, Mutex};

        // More lines than the CenteredModal's inner height can show at once
        // (90%/85% of the harness's fixed 100x30 backend), so the tail of the
        // content is exercised — the same "overflow, then check the last
        // visible row" shape as the palette's height-sweep test.
        let lines: Vec<StyledLine> = (0..40)
            .map(|i| StyledLine::plain(format!("changelog-line-{i:02}")))
            .collect();
        let screen = ChangelogScreen::new(Arc::new(Mutex::new(ChangelogState::Loaded { lines })));

        // Derive the tips needle from the screen itself, like the palette
        // test does, so a locale switch elsewhere in this test binary can't
        // turn this into a flake.
        let tips_text: String = screen.tips()[0]
            .spans
            .iter()
            .map(|s| s.text.clone())
            .collect();
        let tips_needle = tips_text
            .split(' ')
            .next()
            .expect("tips line is non-empty")
            .to_string();

        let buffer = render_paint_screen(
            &screen,
            &ScreenLayout::CenteredModal {
                width_pct: 90,
                height_pct: 85,
                title: Some("Changelog".to_string()),
            },
            Palette::for_theme(Theme::Dark),
            crate::splash::SandboxSplashState::OnDefault,
        );
        let text = buffer_text(&buffer);

        // The harness's TestBackend is 100x30: height_pct 85 -> outer height
        // 25, minus the 1-row top/bottom border margin -> inner height 23.
        // With 40 lines and no scroll, the visible window is lines[0..23], so
        // "changelog-line-22" is the last content row this screen draws.
        let last_visible = "changelog-line-22";
        assert!(
            text.contains(last_visible),
            "the changelog screen's own last visible content row must survive \
             painting, not be hidden by the tips row:\n{text}"
        );

        let content_row = text
            .lines()
            .position(|l| l.contains(last_visible))
            .unwrap_or_else(|| panic!("last content row must paint:\n{text}"));
        let tips_row = text
            .lines()
            .position(|l| l.contains(&tips_needle))
            .unwrap_or_else(|| panic!("the tips row must paint:\n{text}"));
        assert!(
            content_row < tips_row,
            "the changelog screen's last content row {content_row} must paint \
             above the tips row {tips_row} (a modal border row, never inside \
             the content region):\n{text}"
        );

        // A line past the visible window must not appear anywhere — confirms
        // the assertion above is actually exercising a clipped/overflowing
        // buffer, not a coincidentally short one.
        assert!(
            !text.contains("changelog-line-23"),
            "a line past the visible window must not paint:\n{text}"
        );
    }

    /// The blank-panel regression, checked on the painted cells: dropping
    /// the sheet's `> <filter>` header removed the only line this state
    /// used to draw, so without its own empty state the palette would
    /// paint an empty rectangle above the prompt.
    #[tokio::test]
    async fn palette_with_no_matches_paints_an_empty_state() {
        use crate::plugin::builtin::command_palette::screen::{PaletteCommand, PaletteScreen};
        use otto_plugin::KeyCodePortable;

        let mut screen = PaletteScreen::with_commands(vec![PaletteCommand {
            name: "clear".into(),
            description: "clear description".into(),
            needs_arg: false,
        }]);
        for ch in "xyz".chars() {
            screen
                .on_key(KeyEventPortable {
                    code: KeyCodePortable::Char(ch),
                    modifiers: otto_plugin::KeyMods::default(),
                })
                .await
                .expect("palette handles a char key");
        }

        let buffer = render_paint_screen(
            &screen,
            &ScreenLayout::BottomSheet { height: 12 },
            Palette::for_theme(Theme::Dark),
            crate::splash::SandboxSplashState::OnDefault,
        );
        let text = buffer_text(&buffer);

        assert!(
            text.contains(rust_i18n::t!("picker.command-palette.no-matches").as_ref()),
            "a filter matching nothing must paint an empty state, not a blank sheet:\n{text}"
        );
        assert!(
            !text.contains("/clear"),
            "the filtered-out command must not paint:\n{text}"
        );
    }

    #[test]
    fn canvas_content_area_unfocused_is_unchanged() {
        let area = Rect {
            x: 3,
            y: 4,
            width: 20,
            height: 8,
        };
        assert_eq!(canvas_content_area(area, false), area);
    }

    #[test]
    fn canvas_content_area_focused_shrinks_by_one_cell_each_side() {
        let area = Rect {
            x: 3,
            y: 4,
            width: 20,
            height: 8,
        };
        let inner = canvas_content_area(area, true);
        assert_eq!(
            inner,
            Rect {
                x: 4,
                y: 5,
                width: 18,
                height: 6,
            }
        );
    }

    #[test]
    fn canvas_content_area_focused_tiny_area_collapses_without_panic() {
        // 1x1 overlay: the 1-cell border consumes the whole rect, leaving a
        // zero-size inner area (callers must guard against this).
        let area = Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let inner = canvas_content_area(area, true);
        assert_eq!(inner.width, 0);
        assert_eq!(inner.height, 0);
    }

    #[test]
    fn focused_canvas_block_draws_a_border() {
        use ratatui::buffer::Buffer;
        use ratatui::widgets::Widget;

        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 4,
        };
        let mut buf = Buffer::empty(area);
        // Mirror the focus-chrome block the overlay path renders.
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .render(area, &mut buf);
        let has_border = buf
            .content()
            .iter()
            .any(|c| matches!(c.symbol(), "┌" | "┐" | "└" | "┘" | "│" | "─"));
        assert!(has_border, "focused canvas should draw a border");
    }

    #[test]
    fn compose_footer_all_three_groups_populated() {
        let l = vec![rline("Anthropic")];
        let c = vec![rline("idle")];
        let r = vec![rline("cwd")];
        let out = compose_footer_ratatui_line([&l, &c, &r], &rsep());
        assert_eq!(joined_ratatui(&out), "Anthropic · idle · cwd");
    }

    #[test]
    fn compose_footer_only_right_has_no_leading_separator() {
        let empty: Vec<Line<'static>> = vec![];
        let r = vec![rline("cwd")];
        let out = compose_footer_ratatui_line([&empty, &empty, &r], &rsep());
        assert_eq!(joined_ratatui(&out), "cwd");
    }

    #[test]
    fn compose_footer_left_and_right_only_single_separator() {
        let l = vec![rline("Anthropic")];
        let empty: Vec<Line<'static>> = vec![];
        let r = vec![rline("cwd")];
        let out = compose_footer_ratatui_line([&l, &empty, &r], &rsep());
        assert_eq!(joined_ratatui(&out), "Anthropic · cwd");
    }

    #[test]
    fn compose_footer_empty_spans_line_treated_as_no_content() {
        let l = vec![Line::default()];
        let c = vec![rline("idle")];
        let r = vec![rline("cwd")];
        let out = compose_footer_ratatui_line([&l, &c, &r], &rsep());
        assert_eq!(joined_ratatui(&out), "idle · cwd");
    }

    #[test]
    fn compose_footer_all_groups_empty_returns_empty_line() {
        let empty: Vec<Line<'static>> = vec![];
        let out = compose_footer_ratatui_line([&empty, &empty, &empty], &rsep());
        assert!(out.spans.is_empty());
    }

    #[test]
    fn paint_screen_fullscreen_splash_uses_shared_splash_renderer() {
        let first_logo_row =
            crate::splash::shared_content(&crate::splash::SandboxSplashState::OnDefault)
                .into_iter()
                .find(|line| line.kind == crate::splash::SplashLineKind::Logo)
                .expect("logo row in shared content")
                .text;
        let buffer = render_paint_screen(
            &FakeScreen {
                id: "splash".into(),
                body: vec![StyledLine::plain("fake splash body")],
                tips: vec![StyledLine::plain("fake splash tips")],
            },
            &ScreenLayout::Fullscreen { hide_chrome: false },
            palette(),
            crate::splash::SandboxSplashState::OnDefault,
        );
        let text = buffer_text(&buffer);

        assert!(
            text.contains(&first_logo_row),
            "fullscreen splash screen should render the shared startup logo: {text}"
        );
        assert!(
            text.contains("the savvy MCP-native terminal coding agent"),
            "fullscreen splash screen should render the shared startup tagline: {text}"
        );
        assert!(
            text.contains("sandbox: on (use /sandbox off to disable)"),
            "fullscreen splash screen should render the shared sandbox line: {text}"
        );
        assert!(
            text.contains(&format!(
                "press any key to continue · v{}",
                env!("CARGO_PKG_VERSION")
            )),
            "fullscreen splash screen should render the shared versioned hint: {text}"
        );
        assert!(
            !text.contains("fake splash body") && !text.contains("fake splash tips"),
            "shared splash render should ignore plugin-provided body/tips text: {text}"
        );
    }

    #[test]
    fn paint_screen_fullscreen_non_splash_keeps_screen_render_output() {
        let buffer = render_paint_screen(
            &FakeScreen {
                id: "plugins.manager".into(),
                body: vec![StyledLine::plain("fullscreen body")],
                tips: vec![StyledLine::plain("fullscreen tips")],
            },
            &ScreenLayout::Fullscreen { hide_chrome: false },
            palette(),
            crate::splash::SandboxSplashState::OnDefault,
        );
        let text = buffer_text(&buffer);

        assert!(text.contains("fullscreen body"));
        assert!(text.contains("fullscreen tips"));
        assert!(
            !text.contains("the savvy MCP-native terminal coding agent"),
            "non-splash fullscreen screens must not be rerouted through splash rendering: {text}"
        );
    }

    /// A screen that records the exact `Region` it was asked to render into,
    /// so a test can assert on `Screen::render`'s contract directly instead
    /// of inferring it from painted buffer text (which can't distinguish
    /// "the region was correctly shrunk before `render`" from "the region
    /// was left full-size and `tips()` painted over the result" — both
    /// produce an identical final buffer for a fixed-size body).
    struct RegionRecordingScreen {
        tips: Vec<StyledLine>,
        seen_region: std::cell::Cell<Option<Region>>,
    }

    #[async_trait]
    impl Screen for RegionRecordingScreen {
        fn id(&self) -> String {
            "region-recording".into()
        }

        fn render(&self, region: Region) -> Vec<StyledLine> {
            self.seen_region.set(Some(region));
            vec![]
        }

        async fn on_key(&mut self, _key: KeyEventPortable) -> Result<Vec<Effect>, PluginError> {
            Ok(vec![])
        }

        fn tips(&self) -> Vec<StyledLine> {
            self.tips.clone()
        }
    }

    /// Regression for the tips-overpaint fix: `paint_screen` must reserve
    /// the tips row by shrinking the region passed to `Screen::render`
    /// *before* calling it — not by painting over the screen's output
    /// afterward. Checked directly against the region `render` receives
    /// (see `RegionRecordingScreen`) for both layouts that paint a tips row,
    /// rather than inferred from painted output.
    #[test]
    fn paint_screen_reserves_tips_row_out_of_the_region_before_render() {
        let fullscreen = RegionRecordingScreen {
            tips: vec![StyledLine::plain("tips_row_text")],
            seen_region: std::cell::Cell::new(None),
        };
        let _ = render_paint_screen(
            &fullscreen,
            &ScreenLayout::Fullscreen { hide_chrome: false },
            palette(),
            crate::splash::SandboxSplashState::OnDefault,
        );
        // `render_paint_screen`'s TestBackend is 100x30; a non-empty
        // `tips()` must shrink the 30-row frame to a 29-row region.
        let fullscreen_region = fullscreen
            .seen_region
            .get()
            .expect("Fullscreen must call render");
        assert_eq!(
            fullscreen_region.height, 29,
            "Fullscreen render region must exclude the reserved tips row: {fullscreen_region:?}"
        );

        let bottom_sheet = RegionRecordingScreen {
            tips: vec![StyledLine::plain("tips_row_text")],
            seen_region: std::cell::Cell::new(None),
        };
        let _ = render_paint_screen(
            &bottom_sheet,
            &ScreenLayout::BottomSheet { height: 12 },
            palette(),
            crate::splash::SandboxSplashState::OnDefault,
        );
        let sheet_region = bottom_sheet
            .seen_region
            .get()
            .expect("BottomSheet must call render");
        assert_eq!(
            sheet_region.height, 11,
            "BottomSheet render region must exclude the reserved tips row: {sheet_region:?}"
        );

        let no_tips = RegionRecordingScreen {
            tips: vec![],
            seen_region: std::cell::Cell::new(None),
        };
        let _ = render_paint_screen(
            &no_tips,
            &ScreenLayout::Fullscreen { hide_chrome: false },
            palette(),
            crate::splash::SandboxSplashState::OnDefault,
        );
        let no_tips_region = no_tips
            .seen_region
            .get()
            .expect("Fullscreen must call render");
        assert_eq!(
            no_tips_region.height, 30,
            "with no tips to reserve for, render should get the full frame: {no_tips_region:?}"
        );
    }

    #[test]
    fn compose_footer_multiple_contributors_share_a_slot_with_separators() {
        // Two plugins both contributing to `home.footer.left` flow as
        // peers, separated like any other groups.
        let l = vec![rline("Anthropic"), rline("Local")];
        let c = vec![rline("idle")];
        let empty: Vec<Line<'static>> = vec![];
        let out = compose_footer_ratatui_line([&l, &c, &empty], &rsep());
        assert_eq!(joined_ratatui(&out), "Anthropic · Local · idle");
    }

    #[test]
    fn compose_footer_skips_empty_lines_within_a_group() {
        let l = vec![Line::default(), rline("Anthropic")];
        let c = vec![rline("idle")];
        let empty: Vec<Line<'static>> = vec![];
        let out = compose_footer_ratatui_line([&l, &c, &empty], &rsep());
        assert_eq!(joined_ratatui(&out), "Anthropic · idle");
    }

    #[test]
    fn provider_selector_query_row_is_hidden_for_short_catalog_without_query() {
        assert!(!provider_selector_shows_query(
            PROVIDER_SELECTOR_DISCOVERABILITY_THRESHOLD,
            "",
        ));
    }

    #[test]
    fn provider_selector_query_row_is_visible_for_long_catalogs() {
        assert!(provider_selector_shows_query(
            PROVIDER_SELECTOR_DISCOVERABILITY_THRESHOLD + 1,
            "",
        ));
    }

    #[test]
    fn provider_selector_query_row_is_visible_when_filtering_short_catalog() {
        let mut app = fresh_app();
        app.set_provider_query("opn");

        assert!(provider_selector_shows_query(
            effective_providers().len(),
            app.provider_query.as_str(),
        ));
        assert_eq!(
            app.filtered_providers()
                .iter()
                .map(|spec| spec.id)
                .collect::<Vec<_>>(),
            vec!["openai"],
        );
    }

    #[test]
    fn connect_provider_selector_short_list_hides_filter_row() {
        let app = fresh_app();

        let view = build_provider_selector_view(&app, PROVIDER_SELECTOR_DISCOVERABILITY_THRESHOLD);

        assert!(!view.show_query_row);
        assert!(view.empty_state.is_none());
    }

    #[test]
    fn connect_provider_selector_long_list_shows_discoverable_filter_row() {
        let app = fresh_app();

        let view =
            build_provider_selector_view(&app, PROVIDER_SELECTOR_DISCOVERABILITY_THRESHOLD + 1);

        assert!(view.show_query_row);
        assert_eq!(view.query_text.as_deref(), Some("type to filter"));
    }

    #[test]
    fn connect_provider_selector_no_results_renders_empty_state() {
        let mut app = fresh_app();
        app.set_provider_query("zzz");

        let view = build_provider_selector_view(&app, PROVIDER_SELECTOR_DISCOVERABILITY_THRESHOLD);

        assert!(view.show_query_row);
        assert_eq!(
            view.empty_state.as_deref(),
            Some("No providers match `zzz`. Keep typing or press Esc to clear.")
        );
    }

    #[test]
    fn compose_footer_preserves_intra_line_spans() {
        // A single contributor emitting multiple spans (e.g. the
        // home_footer right slot's `cwd · ~N ctx · $0.00 · vX.Y.Z`)
        // must not gain extra separators between its own spans.
        let r = vec![Line::from(vec![
            Span::raw("cwd".to_string()),
            Span::raw(" · ".to_string()),
            Span::raw("~22 ctx".to_string()),
            Span::raw(" · ".to_string()),
            Span::raw("$0.00".to_string()),
        ])];
        let empty: Vec<Line<'static>> = vec![];
        let out = compose_footer_ratatui_line([&empty, &empty, &r], &rsep());
        assert_eq!(joined_ratatui(&out), "cwd · ~22 ctx · $0.00");
    }

    #[test]
    fn footer_turn_state_lines_idle_are_unchanged() {
        let _lock = locale_lock();
        let turn_state = vec![StyledLine::plain("idle")];
        let palette = palette();

        let out = footer_center_lines(&turn_state, Some(0), None, None, 0, palette);
        let expected = vec![crate::plugin::convert::styled_line_to_ratatui(
            turn_state[0].clone(),
            &palette,
        )];

        assert_eq!(out, expected);
    }

    #[test]
    fn footer_turn_state_lines_busy_include_working_label() {
        let _lock = locale_lock();
        let working = rust_i18n::t!("footer.turn-working", id = 3u32).to_string();
        let turn_state = vec![StyledLine::colored(working.clone(), ThemeColor::Accent)];

        let out = footer_center_lines(&turn_state, Some(0), Some(3), None, 0, palette());

        assert_eq!(out.len(), 1);
        assert!(
            joined_ratatui(&out[0]).contains(&working),
            "busy footer should keep the localized working label"
        );
    }

    #[test]
    fn footer_turn_state_lines_busy_override_idle_label_during_submit_gap() {
        let _lock = locale_lock();
        let idle = rust_i18n::t!("footer.idle").to_string();
        let working = rust_i18n::t!("footer.turn-working", id = 3u32).to_string();
        let turn_state = vec![StyledLine::plain(&idle)];

        let out = footer_center_lines(&turn_state, Some(0), None, Some(3), 0, palette());
        let rendered = joined_ratatui(&out[0]);

        assert!(rendered.starts_with(&working));
        assert!(!rendered.starts_with(&idle));
    }

    #[test]
    fn footer_turn_state_lines_busy_include_spinner_glyph_output() {
        let _lock = locale_lock();
        let working = rust_i18n::t!("footer.turn-working", id = 3u32).to_string();
        let turn_state = vec![StyledLine::colored(working.clone(), ThemeColor::Accent)];

        let out = footer_center_lines(&turn_state, Some(0), Some(3), None, 0, palette());
        let rendered = joined_ratatui(&out[0]);
        let spinner = rendered
            .strip_prefix(&format!("{working} "))
            .unwrap_or_default()
            .trim();

        assert!(
            !spinner.is_empty(),
            "expected spinner glyphs after the working label, got: {rendered:?}"
        );
    }

    #[test]
    fn footer_turn_state_lines_busy_use_accent_and_muted_spinner_colors() {
        let _lock = locale_lock();
        let working = rust_i18n::t!("footer.turn-working", id = 3u32).to_string();
        let turn_state = vec![StyledLine::colored(working.clone(), ThemeColor::Accent)];
        let palette = palette();

        let out = footer_center_lines(&turn_state, Some(0), Some(3), None, 0, palette);
        let spinner_spans: Vec<_> = out[0]
            .spans
            .iter()
            .filter(|span| {
                let text = span.content.as_ref();
                !text.trim().is_empty() && text != working
            })
            .collect();

        assert!(
            spinner_spans
                .iter()
                .any(|span| span.style.fg == Some(palette.accent)),
            "expected an accent-colored spinner arc"
        );
        assert!(
            spinner_spans
                .iter()
                .any(|span| span.style.fg == Some(palette.muted)),
            "expected a muted-colored spinner ring"
        );
    }

    #[test]
    fn footer_turn_state_lines_busy_change_spinner_frame_across_ticks() {
        let _lock = locale_lock();
        let working = rust_i18n::t!("footer.turn-working", id = 3u32).to_string();
        let turn_state = vec![StyledLine::colored(working, ThemeColor::Accent)];
        let palette = palette();

        let a = footer_center_lines(&turn_state, Some(0), Some(3), None, 0, palette);
        let b = footer_center_lines(&turn_state, Some(0), Some(3), None, 1, palette);

        assert_ne!(joined_ratatui(&a[0]), joined_ratatui(&b[0]));
    }

    #[test]
    fn footer_turn_state_lines_busy_do_not_invent_text_for_empty_turn_slot() {
        let _lock = locale_lock();
        let turn_state: Vec<StyledLine> = vec![];

        let out = footer_center_lines(&turn_state, None, Some(3), None, 0, palette());

        assert!(out.is_empty());
    }

    #[test]
    fn footer_turn_state_lines_skip_empty_leader_before_attaching_spinner() {
        let _lock = locale_lock();
        let working = rust_i18n::t!("footer.turn-working", id = 3u32).to_string();
        let turn_state = vec![StyledLine { spans: vec![] }, StyledLine::plain(&working)];

        let out = footer_center_lines(&turn_state, Some(1), Some(3), None, 0, palette());
        assert!(out[0].spans.is_empty());

        let rendered = joined_ratatui(&out[1]);
        assert!(rendered.starts_with(&working));
        assert_ne!(rendered, working);
    }

    // Canvas overlay row math --------------------------------------------

    #[test]
    fn wrapped_row_count_empty_line_is_one_row() {
        assert_eq!(wrapped_row_count(0, 80), 1);
    }

    #[test]
    fn wrapped_row_count_short_line_is_one_row() {
        assert_eq!(wrapped_row_count(10, 80), 1);
    }

    #[test]
    fn wrapped_row_count_exact_width_is_one_row() {
        assert_eq!(wrapped_row_count(80, 80), 1);
    }

    #[test]
    fn wrapped_row_count_overflow_by_one_yields_two_rows() {
        assert_eq!(wrapped_row_count(81, 80), 2);
    }

    #[test]
    fn wrapped_row_count_two_full_widths_plus_one_yields_three() {
        assert_eq!(wrapped_row_count(161, 80), 3);
    }

    #[test]
    fn wrapped_row_count_zero_wrap_width_collapses_to_one_row() {
        // Defensive: a 0-width inner area happens during terminal resize
        // edge cases. Returning 1 keeps the cumulative sum monotonic and
        // avoids divide-by-zero downstream.
        assert_eq!(wrapped_row_count(100, 0), 1);
    }

    #[test]
    fn split_top_one_line_splits_a_tall_rect() {
        let r = Rect::new(2, 3, 80, 10);
        let (top, body) = split_top_one_line(r);
        assert_eq!(top, Rect::new(2, 3, 80, 1));
        assert_eq!(body, Rect::new(2, 4, 80, 9));
    }

    #[test]
    fn split_top_one_line_with_one_row_leaves_no_body() {
        let r = Rect::new(0, 0, 40, 1);
        let (top, body) = split_top_one_line(r);
        assert_eq!(top.height, 1);
        assert_eq!(body.height, 0);
    }

    #[test]
    fn split_top_one_line_with_zero_rows_returns_unchanged() {
        let r = Rect::new(0, 0, 40, 0);
        let (top, body) = split_top_one_line(r);
        assert_eq!(top, r);
        assert_eq!(body, r);
    }

    /// A bottom sheet sits directly above the prompt, not at the bottom of
    /// the frame: on a roomy terminal the full requested height fits and the
    /// sheet's last row is the one immediately above `input_top`.
    #[test]
    fn bottom_sheet_is_anchored_above_the_prompt() {
        // 40-row frame, 3-row prompt + 1-row footer => input_top = 36.
        let area = Rect::new(0, 0, 100, 40);
        let sheet = bottom_sheet_rect(area, 36, 12);
        assert_eq!(sheet, Rect::new(0, 24, 100, 12));
        assert_eq!(sheet.y + sheet.height, 36, "must stop at the prompt");
    }

    /// Regression: on a short terminal there is less room above the prompt
    /// than the sheet asks for, and the sheet must shrink rather than grow
    /// back down over the header and the textarea.
    #[test]
    fn bottom_sheet_shrinks_instead_of_covering_the_prompt() {
        // 15-row frame: header 3 + log 1 + banner 1 + tips 1 + input 3 +
        // footer 1 leaves input_top = 11, well under the requested 12.
        let area = Rect::new(0, 0, 100, 15);
        let sheet = bottom_sheet_rect(area, 11, 12);
        assert_eq!(sheet.y, 0, "clamped sheet starts at the top of the area");
        assert_eq!(sheet.height, 11);
        assert_eq!(
            sheet.y + sheet.height,
            11,
            "sheet must never extend past input_top"
        );
    }

    /// The sheet is positioned relative to `area`, not the screen origin.
    #[test]
    fn bottom_sheet_respects_a_nonzero_area_origin() {
        let area = Rect::new(4, 5, 60, 30);
        let sheet = bottom_sheet_rect(area, 30, 8);
        assert_eq!(sheet, Rect::new(4, 22, 60, 8));
    }

    /// Degenerate case: the prompt is at the very top of the area, so there
    /// is no room at all. An empty sheet is fine; an overlapping one is not.
    #[test]
    fn bottom_sheet_with_no_room_above_the_prompt_is_empty() {
        let area = Rect::new(0, 7, 80, 20);
        let sheet = bottom_sheet_rect(area, 7, 12);
        assert_eq!(sheet.height, 0);
        assert_eq!(sheet.y, 7);
    }

    // --- paint_ghost_completion tests (issue #118) ---

    fn ghost_style() -> Style {
        Style::default().fg(Color::Gray)
    }

    fn cells_to_string(buf: &Buffer, y: u16, x_range: std::ops::Range<u16>) -> String {
        x_range.map(|x| buf[(x, y)].symbol().to_string()).collect()
    }

    /// A fitting single line draws the ghost text at `inner.x + cursor.1`,
    /// styled with the passed-in style.
    #[test]
    fn paint_ghost_completion_draws_at_the_cursor_when_the_line_fits() {
        let inner = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(inner);
        paint_ghost_completion(
            &mut buf,
            inner,
            (0, 3),
            &["/co".to_string()],
            "nnect",
            ghost_style(),
        );
        assert_eq!(cells_to_string(&buf, 0, 3..8), "nnect");
        assert_eq!(buf[(3, 0)].fg, Color::Gray);
    }

    /// Regression for the wrap-guard gap a design critique found: a line
    /// wider than the interior width must not be trusted to still be on
    /// visual row 0, so no ghost text may be painted for it — even though
    /// `cursor.0 == 0` and `lines.len() == 1` both hold. The cursor column
    /// here (`2`) is deliberately kept *within* `inner`'s width — if it
    /// were past the width (e.g. at the line's own end), the unrelated
    /// `x >= inner.x + inner.width` bounds check downstream would also
    /// reject the paint, masking whether `line_fits` is doing anything at
    /// all. Removing the `line_fits` conjunct from `paint_ghost_completion`
    /// makes this exact test fail (verified while writing it); keep it
    /// that way.
    #[test]
    fn paint_ghost_completion_skips_a_line_that_does_not_fit() {
        let inner = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(inner);
        paint_ghost_completion(
            &mut buf,
            inner,
            (0, 2),
            &["/clearclearclear".to_string()],
            "x",
            ghost_style(),
        );
        for x in 0..5 {
            assert_eq!(buf[(x, 0)].symbol(), " ", "buffer must be untouched");
        }
    }

    /// Multi-line `lines` (a pasted newline, however that might happen)
    /// must not be treated as a safe single visual row even when the
    /// cursor happens to report row 0.
    #[test]
    fn paint_ghost_completion_skips_multiline_content() {
        let inner = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(inner);
        paint_ghost_completion(
            &mut buf,
            inner,
            (0, 3),
            &["/co".to_string(), "second line".to_string()],
            "nnect",
            ghost_style(),
        );
        for x in 0..inner.width {
            assert_eq!(buf[(x, 0)].symbol(), " ");
        }
    }

    /// A cursor reported on a later logical line is never safe to paint at
    /// row 0.
    #[test]
    fn paint_ghost_completion_skips_when_cursor_row_is_not_zero() {
        let inner = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(inner);
        paint_ghost_completion(
            &mut buf,
            inner,
            (1, 3),
            &["/co".to_string()],
            "nnect",
            ghost_style(),
        );
        for x in 0..inner.width {
            assert_eq!(buf[(x, 0)].symbol(), " ");
        }
    }

    /// Ghost text longer than the remaining width clips via `max_width`
    /// rather than panicking or writing past the interior rect.
    #[test]
    fn paint_ghost_completion_clips_to_the_remaining_width() {
        let inner = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(inner);
        paint_ghost_completion(
            &mut buf,
            inner,
            (0, 8),
            &["/connectno".to_string()],
            "nnectnow",
            ghost_style(),
        );
        assert_eq!(cells_to_string(&buf, 0, 8..10), "nn");
    }

    /// Regression for a reachable panic a security review caught: on a
    /// squeezed layout (a very short terminal), `Block::inner` can clamp
    /// the prompt's interior rect's `y` to the frame's own bottom edge —
    /// one row past the buffer's actual area. Without clamping `inner` to
    /// `buf.area` first, `Buffer::set_stringn` would panic (its cell write
    /// indexes unconditionally, not gated behind `debug_assertions`). Here
    /// `inner` is deliberately one row below `buf`'s 4-row area; this must
    /// not panic, and must not touch the buffer at all.
    #[test]
    fn paint_ghost_completion_does_not_panic_when_inner_is_outside_the_buffer() {
        let buf_area = Rect::new(0, 0, 20, 4);
        let mut buf = Buffer::empty(buf_area);
        let out_of_buffer_inner = Rect::new(0, 4, 20, 1);
        paint_ghost_completion(
            &mut buf,
            out_of_buffer_inner,
            (0, 3),
            &["/co".to_string()],
            "nnect",
            ghost_style(),
        );
        for y in 0..buf_area.height {
            for x in 0..buf_area.width {
                assert_eq!(buf[(x, y)].symbol(), " ");
            }
        }
    }

    /// A degenerate zero-height `inner` (still within `buf`'s area, e.g. a
    /// layout constraint that collapsed to nothing) must also be a safe
    /// no-op rather than reaching the cursor/width arithmetic below.
    #[test]
    fn paint_ghost_completion_does_not_panic_on_zero_height_inner() {
        let buf_area = Rect::new(0, 0, 20, 4);
        let mut buf = Buffer::empty(buf_area);
        let zero_height_inner = Rect::new(0, 1, 20, 0);
        paint_ghost_completion(
            &mut buf,
            zero_height_inner,
            (0, 3),
            &["/co".to_string()],
            "nnect",
            ghost_style(),
        );
        for y in 0..buf_area.height {
            for x in 0..buf_area.width {
                assert_eq!(buf[(x, y)].symbol(), " ");
            }
        }
    }
}
