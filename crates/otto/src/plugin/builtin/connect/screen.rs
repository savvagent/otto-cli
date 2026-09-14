//! Provider picker — choose from registered provider plugins.

use async_trait::async_trait;
use otto_plugin::{
    Effect, KeyCodePortable, KeyEventPortable, PluginError, ProviderId, Region, Screen, StyledLine,
    StyledSpan, ThemeColor,
};

use crate::providers::{PROVIDER_SELECTOR_DISCOVERABILITY_THRESHOLD, provider_label_matches_query};

/// Provider picker screen.
///
/// v0.9 ships an empty fallback list. PR 6 wires registered-provider
/// discovery via `HostEvent::ProviderRegistered` + an in-memory cache
/// here. For PR 5 the screen renders "no providers registered yet".
#[derive(Debug)]
pub struct ConnectPickerScreen {
    candidates: Vec<(ProviderId, String)>,
    filtered: Vec<usize>,
    query: String,
    active_provider_id: Option<ProviderId>,
    cursor: usize,
}

impl ConnectPickerScreen {
    /// Construct a picker with no pre-loaded candidates (PR 5 default).
    pub fn new() -> Self {
        Self {
            candidates: vec![],
            filtered: vec![],
            query: String::new(),
            active_provider_id: None,
            cursor: 0,
        }
    }

    /// Public constructor used by `ConnectPlugin::create_screen` (and by
    /// tests) to inject the registered-provider set into a freshly opened
    /// picker. `ConnectPlugin` accumulates candidates via
    /// [`Plugin::on_event`] on [`otto_plugin::HostEvent::ProviderRegistered`].
    pub fn with_candidates(candidates: Vec<(ProviderId, String)>) -> Self {
        Self::with_candidates_and_active(candidates, None)
    }

    pub fn with_candidates_and_active(
        candidates: Vec<(ProviderId, String)>,
        active_provider_id: Option<ProviderId>,
    ) -> Self {
        let filtered = (0..candidates.len()).collect();
        let cursor = active_provider_id
            .as_ref()
            .and_then(|active| {
                candidates
                    .iter()
                    .position(|(candidate_id, _)| candidate_id == active)
            })
            .unwrap_or(0);
        Self {
            candidates,
            filtered,
            query: String::new(),
            active_provider_id,
            cursor,
        }
    }

    fn refresh_filtered(&mut self) {
        let previous = self.selected_candidate().map(|(id, _)| id.clone());
        self.filtered = self
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, (id, display))| {
                provider_label_matches_query(id.as_str(), display, &self.query)
            })
            .map(|(idx, _)| idx)
            .collect();

        if self.query.is_empty() {
            self.reset_cursor_for_active();
            return;
        }

        if let Some(previous) = previous {
            if let Some(idx) = self
                .filtered
                .iter()
                .position(|candidate_idx| self.candidates[*candidate_idx].0 == previous)
            {
                self.cursor = idx;
                return;
            }
        }

        self.cursor = 0;
    }

    fn reset_cursor_for_active(&mut self) {
        self.cursor = self
            .active_provider_id
            .as_ref()
            .and_then(|active| {
                self.filtered
                    .iter()
                    .position(|candidate_idx| &self.candidates[*candidate_idx].0 == active)
            })
            .unwrap_or(0);
    }

    fn selected_candidate(&self) -> Option<&(ProviderId, String)> {
        self.filtered
            .get(self.cursor)
            .and_then(|idx| self.candidates.get(*idx))
    }

    fn show_query(&self) -> bool {
        !self.query.is_empty()
            || self.candidates.len() > PROVIDER_SELECTOR_DISCOVERABILITY_THRESHOLD
    }
}

impl Default for ConnectPickerScreen {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Screen for ConnectPickerScreen {
    fn id(&self) -> String {
        "connect.picker".to_string()
    }

    fn render(&self, _region: Region) -> Vec<StyledLine> {
        if self.candidates.is_empty() {
            return vec![
                StyledLine::plain(rust_i18n::t!("picker.connect.no-providers").to_string()),
                StyledLine::plain(""),
                StyledLine::colored(
                    rust_i18n::t!("picker.connect.open-plugins-hint").to_string(),
                    ThemeColor::Warning,
                ),
            ];
        }
        let mut lines = Vec::new();
        if self.show_query() {
            lines.push(StyledLine {
                spans: vec![
                    StyledSpan::muted("Search: "),
                    StyledSpan::colored(
                        if self.query.is_empty() {
                            "type to filter".into()
                        } else {
                            self.query.clone()
                        },
                        if self.query.is_empty() {
                            ThemeColor::Muted
                        } else {
                            ThemeColor::Fg
                        },
                    ),
                ],
            });
        }

        if self.filtered.is_empty() {
            lines.push(StyledLine::plain(""));
            lines.push(StyledLine {
                spans: vec![
                    StyledSpan::muted("No providers match"),
                    StyledSpan::colored(format!(" {}", self.query), ThemeColor::Accent),
                ],
            });
            return lines;
        }

        lines.extend(self.filtered.iter().enumerate().map(|(i, idx)| {
            let (id, display) = &self.candidates[*idx];
            let marker = if i == self.cursor { "▶ " } else { "  " };
            StyledLine::plain(format!("{marker}{display}  ({})", id.as_str()))
        }));
        lines
    }

    async fn on_key(&mut self, key: KeyEventPortable) -> Result<Vec<Effect>, PluginError> {
        match key.code {
            KeyCodePortable::Esc if self.query.is_empty() => Ok(vec![Effect::CloseScreen]),
            KeyCodePortable::Esc => {
                self.query.clear();
                self.refresh_filtered();
                Ok(vec![])
            }
            KeyCodePortable::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                Ok(vec![])
            }
            KeyCodePortable::Down => {
                let max = self.filtered.len().saturating_sub(1);
                if self.cursor < max {
                    self.cursor += 1;
                }
                Ok(vec![])
            }
            KeyCodePortable::Backspace => {
                self.query.pop();
                self.refresh_filtered();
                Ok(vec![])
            }
            KeyCodePortable::Char(c) => {
                self.query.push(c);
                self.refresh_filtered();
                Ok(vec![])
            }
            KeyCodePortable::Enter => {
                let Some((pid, _)) = self.selected_candidate().cloned() else {
                    return Ok(vec![]);
                };
                let name = format!("connect {}", pid.as_str());
                Ok(vec![Effect::Stack(vec![
                    Effect::CloseScreen,
                    Effect::RunSlash { name, args: vec![] },
                ])])
            }
            _ => Ok(vec![]),
        }
    }

    fn tips(&self) -> Vec<StyledLine> {
        vec![StyledLine::plain(format!(
            "{} · type to filter · Backspace delete · Esc clear/cancel",
            rust_i18n::t!("picker.connect.tips")
        ))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use otto_plugin::KeyMods;

    fn key(c: KeyCodePortable) -> KeyEventPortable {
        KeyEventPortable {
            code: c,
            modifiers: KeyMods::default(),
        }
    }

    #[tokio::test]
    async fn empty_candidates_renders_helper_text() {
        let s = ConnectPickerScreen::new();
        let lines = s.render(Region {
            x: 0,
            y: 0,
            width: 60,
            height: 10,
        });
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains(rust_i18n::t!("picker.connect.no-providers").as_ref()),
            "expected no-providers text, got: {joined}"
        );
        assert!(
            joined.contains(rust_i18n::t!("picker.connect.open-plugins-hint").as_ref()),
            "expected open-plugins-hint text, got: {joined}"
        );
        // Pins span colours so the #117 constructor rewrite cannot change them silently.
        // The no-providers line and the blank spacer line are unstyled (fg: None);
        // the "open plugins" hint is deliberately Warning-coloured, NOT Muted, even
        // though it reads like an ordinary hint — a mechanical rewrite to `muted()`
        // here would silently change its colour.
        assert_eq!(lines[0].spans[0].fg, None);
        assert_eq!(lines[1].spans[0].fg, None);
        assert_eq!(lines[2].spans[0].fg, Some(ThemeColor::Warning));
    }

    #[tokio::test]
    async fn enter_with_candidate_routes_to_connect_provider_slash() {
        let mut s = ConnectPickerScreen::with_candidates(vec![(
            ProviderId::new("anthropic").unwrap(),
            "Anthropic".into(),
        )]);
        let effs = s.on_key(key(KeyCodePortable::Enter)).await.unwrap();
        match &effs[0] {
            Effect::Stack(children) => {
                assert!(matches!(children[0], Effect::CloseScreen));
                match &children[1] {
                    Effect::RunSlash { name, .. } => assert_eq!(name, "connect anthropic"),
                    _ => panic!(),
                }
            }
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn alt_enter_routes_identically_to_plain_enter() {
        let mut s = ConnectPickerScreen::with_candidates(vec![(
            ProviderId::new("anthropic").unwrap(),
            "Anthropic".into(),
        )]);
        let mut k = key(KeyCodePortable::Enter);
        k.modifiers.alt = true;
        let effs = s.on_key(k).await.unwrap();
        match &effs[0] {
            Effect::Stack(children) => {
                assert!(matches!(children[0], Effect::CloseScreen));
                match &children[1] {
                    Effect::RunSlash { name, args } => {
                        assert_eq!(name, "connect anthropic");
                        assert!(
                            args.is_empty(),
                            "Alt+Enter must no longer emit --rekey — every stored-key case \
                             already opens the modal; got args: {args:?}"
                        );
                    }
                    _ => panic!(),
                }
            }
            _ => panic!(),
        }
    }

    #[test]
    fn short_list_hides_query_until_user_types() {
        let s = ConnectPickerScreen::with_candidates(vec![
            (ProviderId::new("anthropic").unwrap(), "Anthropic".into()),
            (ProviderId::new("gemini").unwrap(), "Google Gemini".into()),
        ]);
        let lines = s.render(Region {
            x: 0,
            y: 0,
            width: 60,
            height: 10,
        });
        let joined = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!joined.contains("Search:"));
    }

    #[tokio::test]
    async fn typing_filters_actual_connect_picker() {
        let mut s = ConnectPickerScreen::with_candidates(vec![
            (
                ProviderId::new("anthropic").unwrap(),
                "Anthropic (Claude)".into(),
            ),
            (ProviderId::new("gemini").unwrap(), "Google Gemini".into()),
            (ProviderId::new("openai").unwrap(), "OpenAI".into()),
            (ProviderId::new("deepseek").unwrap(), "DeepSeek".into()),
            (ProviderId::new("local").unwrap(), "Ollama (local)".into()),
        ]);
        s.on_key(key(KeyCodePortable::Char('o'))).await.unwrap();
        s.on_key(key(KeyCodePortable::Char('p'))).await.unwrap();
        s.on_key(key(KeyCodePortable::Char('n'))).await.unwrap();

        let lines = s.render(Region {
            x: 0,
            y: 0,
            width: 60,
            height: 10,
        });
        let joined = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Search: "), "rendered: {joined}");
        assert!(joined.contains("opn"), "rendered: {joined}");
        assert!(joined.contains("OpenAI"), "rendered: {joined}");
        assert!(!joined.contains("DeepSeek"), "rendered: {joined}");
    }

    #[tokio::test]
    async fn enter_uses_filtered_candidate() {
        let mut s = ConnectPickerScreen::with_candidates(vec![
            (
                ProviderId::new("anthropic").unwrap(),
                "Anthropic (Claude)".into(),
            ),
            (ProviderId::new("openai").unwrap(), "OpenAI".into()),
        ]);
        s.on_key(key(KeyCodePortable::Char('o'))).await.unwrap();
        s.on_key(key(KeyCodePortable::Char('p'))).await.unwrap();
        s.on_key(key(KeyCodePortable::Char('n'))).await.unwrap();

        let effs = s.on_key(key(KeyCodePortable::Enter)).await.unwrap();
        match &effs[0] {
            Effect::Stack(children) => match &children[1] {
                Effect::RunSlash { name, .. } => assert_eq!(name, "connect openai"),
                other => panic!("expected RunSlash, got {other:?}"),
            },
            other => panic!("expected Stack, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_match_keeps_picker_open() {
        let mut s = ConnectPickerScreen::with_candidates(vec![(
            ProviderId::new("anthropic").unwrap(),
            "Anthropic".into(),
        )]);
        s.on_key(key(KeyCodePortable::Char('z'))).await.unwrap();

        let lines = s.render(Region {
            x: 0,
            y: 0,
            width: 60,
            height: 10,
        });
        let joined = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("No providers match"), "rendered: {joined}");
        // Pins span colours so the #117 constructor rewrite cannot change them silently.
        // "No providers match" (Muted) and the trailing " <query>" (Accent) are two
        // different colours on the same line — an easy pair to collapse into one
        // colour by mistake during a mechanical rewrite.
        let no_match_line = lines
            .iter()
            .find(|l| {
                l.spans
                    .first()
                    .is_some_and(|s| s.text == "No providers match")
            })
            .expect("no-match line");
        assert_eq!(no_match_line.spans[0].fg, Some(ThemeColor::Muted));
        assert_eq!(no_match_line.spans[1].fg, Some(ThemeColor::Accent));
        assert!(
            s.on_key(key(KeyCodePortable::Enter))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn escape_clears_query_before_closing() {
        let mut s = ConnectPickerScreen::with_candidates(vec![(
            ProviderId::new("anthropic").unwrap(),
            "Anthropic".into(),
        )]);
        s.on_key(key(KeyCodePortable::Char('a'))).await.unwrap();

        assert!(
            s.on_key(key(KeyCodePortable::Esc))
                .await
                .unwrap()
                .is_empty()
        );
        let second = s.on_key(key(KeyCodePortable::Esc)).await.unwrap();
        assert!(matches!(second.as_slice(), [Effect::CloseScreen]));
    }

    #[tokio::test]
    async fn clearing_query_restores_active_provider() {
        let mut s = ConnectPickerScreen::with_candidates_and_active(
            vec![
                (ProviderId::new("anthropic").unwrap(), "Anthropic".into()),
                (ProviderId::new("openai").unwrap(), "OpenAI".into()),
            ],
            Some(ProviderId::new("openai").unwrap()),
        );

        assert_eq!(
            s.selected_candidate().map(|(id, _)| id.as_str()),
            Some("openai")
        );

        s.on_key(key(KeyCodePortable::Char('n'))).await.unwrap();
        s.on_key(key(KeyCodePortable::Char('t'))).await.unwrap();
        assert_eq!(
            s.selected_candidate().map(|(id, _)| id.as_str()),
            Some("anthropic")
        );

        s.on_key(key(KeyCodePortable::Backspace)).await.unwrap();
        s.on_key(key(KeyCodePortable::Backspace)).await.unwrap();
        assert_eq!(
            s.selected_candidate().map(|(id, _)| id.as_str()),
            Some("openai")
        );
    }

    // Pins span colours so the #117 constructor rewrite cannot change them silently.
    #[test]
    fn search_prefix_is_muted_and_placeholder_is_muted_when_query_empty() {
        // 6 candidates exceeds PROVIDER_SELECTOR_DISCOVERABILITY_THRESHOLD (5), so
        // `show_query()` is true even with an empty query.
        let s = ConnectPickerScreen::with_candidates(vec![
            (ProviderId::new("a").unwrap(), "A".into()),
            (ProviderId::new("b").unwrap(), "B".into()),
            (ProviderId::new("c").unwrap(), "C".into()),
            (ProviderId::new("d").unwrap(), "D".into()),
            (ProviderId::new("e").unwrap(), "E".into()),
            (ProviderId::new("f").unwrap(), "F".into()),
        ]);
        let lines = s.render(Region {
            x: 0,
            y: 0,
            width: 60,
            height: 10,
        });
        let search_line = &lines[0];
        assert_eq!(search_line.spans[0].text, "Search: ");
        assert_eq!(search_line.spans[0].fg, Some(ThemeColor::Muted));
        assert_eq!(search_line.spans[1].text, "type to filter");
        assert_eq!(search_line.spans[1].fg, Some(ThemeColor::Muted));
    }

    #[tokio::test]
    async fn search_prefix_stays_muted_and_value_uses_fg_when_query_typed() {
        let mut s = ConnectPickerScreen::with_candidates(vec![
            (
                ProviderId::new("anthropic").unwrap(),
                "Anthropic (Claude)".into(),
            ),
            (ProviderId::new("openai").unwrap(), "OpenAI".into()),
        ]);
        s.on_key(key(KeyCodePortable::Char('o'))).await.unwrap();

        let lines = s.render(Region {
            x: 0,
            y: 0,
            width: 60,
            height: 10,
        });
        let search_line = &lines[0];
        assert_eq!(search_line.spans[0].text, "Search: ");
        assert_eq!(search_line.spans[0].fg, Some(ThemeColor::Muted));
        assert_eq!(search_line.spans[1].text, "o");
        // Once the user has typed, the value span switches from Muted (placeholder)
        // to Fg (real content) — losing that switch would make typed text look
        // permanently greyed out.
        assert_eq!(search_line.spans[1].fg, Some(ThemeColor::Fg));
    }
}
