//! Internal Markdown rendering plugin using ratskin (termimad).

use async_trait::async_trait;
use otto_plugin::{Contributions, Manifest, Plugin, PluginId, PluginKind, SystemPromptSegment};

/// `internal:markdown` plugin. Constructed by `register_builtins`.
#[derive(Debug, Default)]
pub struct MarkdownPlugin;

impl MarkdownPlugin {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Plugin for MarkdownPlugin {
    fn manifest(&self) -> Manifest {
        let mut contributions = Contributions::default();
        // No `ContentRendererSpec`: Markdown is rendered at draw time by
        // `crate::ui` into styled `Line`s (see the module's `post_process`
        // and `renderer`), not through the pixel-frame `ContentRenderer`
        // path the SPP content-block dispatch uses. Claiming the `"md"`
        // block type here would advertise a renderer that never exists and
        // could collide with a future canonical claim.
        contributions.prompt_segments = vec![SystemPromptSegment {
            id: "internal:markdown:default".to_string(),
            text: "When you want to display nicely formatted content (especially tables), \
                   wrap it in a ```md fenced block. The terminal will render it with rich formatting."
                .to_string(),
        }];
        Manifest {
            id: PluginId::new("internal:markdown").expect("valid built-in id"),
            name: "Markdown Renderer".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            description: "Renders ```md fenced blocks using ratskin for rich inline display"
                .to_string(),
            kind: PluginKind::Optional,
            contributions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_advertises_prompt_segment_only() {
        let m = MarkdownPlugin::new().manifest();
        assert_eq!(m.id, PluginId::new("internal:markdown").unwrap());
        assert_eq!(m.kind, PluginKind::Optional);
        // Markdown renders via the transcript text path, not the SPP
        // content-block/`ContentRenderer` path, so it must not claim a
        // canonical block type.
        assert!(m.contributions.content_renderers.is_empty());
        assert_eq!(m.contributions.prompt_segments.len(), 1);
        assert_eq!(
            m.contributions.prompt_segments[0].id,
            "internal:markdown:default"
        );
    }
}
