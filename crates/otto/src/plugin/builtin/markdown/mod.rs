//! `internal:markdown` plugin
//!
//! Renders ` ```md ` fenced blocks using `ratskin` (termimad) for rich
//! inline Markdown display in the transcript (headings, tables, lists,
//! code, quotes, links).

pub mod plugin;
pub mod post_process;
pub mod renderer;

pub use plugin::MarkdownPlugin;
