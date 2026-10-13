//! `internal:user-skills` — discovers Claude-Code-compatible skills from
//! `.otto/skills/` and `.claude/skills/` and exposes them to the model
//! under progressive disclosure, plus to the user via `/skills`.
//!
//! Three levels, matching the Claude Code format:
//!
//! 1. **Level 1** — name + description for every skill, rendered by
//!    [`index::SkillIndex::catalog`] into a single system-prompt segment.
//! 2. **Level 2** — the full `SKILL.md` body, returned as a tool result
//!    on invocation. Bodies never enter the system prompt.
//! 3. **Level 3** — bundled `scripts/` / `references/` / `assets/`, read
//!    on demand through `tool-fs` / `tool-bash` from the skill root.
//!
//! See `docs/superpowers/specs/2026-09-09-issue-83-claude-code-compat-design.md`.
//!
//! Discovery walks the four tiers the spec defines — project beats user,
//! `.otto/` beats `.claude/` — plus `<project>/.github/skills/`, which
//! Copilot CLI defines and which repos predating otto already use. `.github/` ranks last among the project tiers and has no
//! user-scope counterpart, since Copilot CLI only defines it in-repo.
//!
//! For the level-3 trust gate `.github/` counts as project scope like the
//! other two: its skills arrive with the checkout, which is the exposure
//! the gate exists for.

pub mod discovery;
pub mod frontmatter;
pub mod spec;

pub mod index;
pub mod skill_tool;
pub mod trust;

use std::path::PathBuf;
use std::sync::RwLock as StdRwLock;

use async_trait::async_trait;
use otto_plugin::{
    Contributions, Effect, HookKind, HostEvent, Manifest, Plugin, PluginError, PluginId,
    PluginKind, ScreenArgs, SlashSpec, StyledLine, SystemPromptSegment,
};

use crate::plugin::builtin::user_skills::discovery::discover;
use crate::plugin::builtin::user_skills::trust::SkillTrust;

pub use index::SkillIndex;
// Not consumed outside this module yet (no interactive picker was built —
// see the E1 design spec's corrected Invocation section). Kept `pub` for
// E2, which needs `SkillScope::Plugin` to register plugin-bundled skills
// from outside this module.
#[allow(unused_imports)]
pub use spec::{SkillScope, SkillSpec, ToolScope};

/// Stable id for the level-1 catalog's system-prompt segment. `None`
/// while `catalog_cache` holds `None` — a user with zero skills gets no
/// segment at all (spec acceptance criterion 3).
const CATALOG_SEGMENT_ID: &str = "internal:user-skills:catalog";

/// Ceiling on one description shown by `/skills`' listing, in chars.
/// Distinct from [`index::MAX_DESCRIPTION_CHARS`] (the level-1 catalog's
/// own, larger cap): a picker line is meant to be scanned at a glance,
/// so it truncates harder than the system-prompt catalog does.
const LISTING_DESCRIPTION_CHARS: usize = 200;

pub struct UserSkillsPlugin {
    project_root: PathBuf,
    user_home: PathBuf,
    index: SkillIndex,
    /// Rendered level-1 catalog, kept in sync with `index` by every
    /// caller that mutates it (`HostStarting`, `/reload-skills`).
    /// `std::sync::RwLock`, not `tokio::sync::RwLock`: `Plugin::manifest`
    /// is synchronous and reads this without an `.await`. No `Arc` here —
    /// `UserSkillsPlugin` is owned exclusively behind a `Box<dyn Plugin>`
    /// and never shared, so a bare lock is sufficient.
    catalog_cache: StdRwLock<Option<String>>,
    /// Count of malformed `SKILL.md` files from the most recent
    /// discovery pass, surfaced by `/skills`' warning note. Cached
    /// rather than recomputed because `/skills` (no arg) reads `index`
    /// only — it does not re-run `discover()`.
    last_warning_count: usize,
    /// Shared with `App::trust_levels` (via `internal:user-slash-commands`'
    /// own `TrustMap`), read under a read-lock by the level-3 trust gate.
    /// This is the same live, in-memory map `user_slash_commands` reads —
    /// not a fresh disk read per call — so a `SessionTextOnly` decision
    /// made in the trust modal for one of them is honored by the other in
    /// the same session, and neither re-reopens the modal on every call.
    trust_levels: crate::plugin::builtin::user_slash_commands::TrustMap,
}

impl UserSkillsPlugin {
    pub fn new() -> Self {
        let project_root = std::env::current_dir().unwrap_or_else(|error| {
            tracing::warn!("user-skills: failed to resolve current directory: {error}");
            PathBuf::from(".")
        });
        let user_home = dirs::home_dir().unwrap_or_else(|| {
            tracing::warn!("user-skills: failed to resolve home directory");
            PathBuf::from(".")
        });
        // Not used by real app wiring (`register_builtins` always calls
        // `with_roots` with the shared `App::trust_levels` map) — this is
        // a convenience constructor, so a fresh, empty map is fine here.
        Self::with_roots(
            project_root,
            user_home,
            std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::BTreeMap::new())),
        )
    }

    /// Both roots are explicit so tests can point the user tiers at a
    /// tempdir instead of the developer's real `~/.claude/skills/`.
    /// `trust_levels` is the same shared map `internal:user-slash-commands`
    /// reads and writes, so the two plugins never disagree about whether a
    /// project has been trusted this session.
    pub fn with_roots(
        project_root: PathBuf,
        user_home: PathBuf,
        trust_levels: crate::plugin::builtin::user_slash_commands::TrustMap,
    ) -> Self {
        Self {
            project_root,
            user_home,
            index: SkillIndex::empty(),
            catalog_cache: StdRwLock::new(None),
            last_warning_count: 0,
            trust_levels,
        }
    }

    /// Build the `skill` tool's registration effect, or none while no
    /// skill has been discovered yet — mirrors
    /// `UserAgentsPlugin::register_task_tool_effects`.
    async fn register_skill_tool_effects(&self) -> Vec<Effect> {
        if self.index.is_empty().await {
            return vec![];
        }
        let spec = skill_tool::build_tool_def(&self.index).await;
        let handler = skill_tool::handler_arc(
            self.index.clone(),
            self.project_root.clone(),
            self.trust_levels.clone(),
        );
        vec![Effect::RegisterInProcessTool { spec, handler }]
    }

    /// Re-run discovery, replace `index`, and refresh `catalog_cache` to
    /// match. Shared by `HostStarting` and `/reload-skills` so the two
    /// call sites can never drift on what "refresh" means. Returns the
    /// number of skills now indexed, for the caller's count-reporting note.
    async fn rediscover(&mut self) -> usize {
        let discovered = discover(&self.project_root, &self.user_home);
        self.last_warning_count = discovered.warnings.len();
        if !discovered.warnings.is_empty() {
            tracing::warn!(
                "user-skills: skipped {} invalid skill definition(s) during discovery: {:?}",
                discovered.warnings.len(),
                discovered.warnings
            );
        }
        self.index.replace(discovered.skills).await;
        let catalog = self.index.catalog().await;
        match self.catalog_cache.write() {
            Ok(mut guard) => *guard = catalog,
            Err(poisoned) => *poisoned.into_inner() = catalog,
        }
        self.index.len().await
    }

    /// Truncate on a char boundary for the `/skills` listing. Separate
    /// from `index::truncate_chars` (private to that module, and tuned
    /// to the larger system-prompt cap) rather than shared, since the two
    /// callers truncate to different lengths for different audiences.
    fn truncate_for_listing(s: &str) -> String {
        if s.chars().count() <= LISTING_DESCRIPTION_CHARS {
            return s.to_string();
        }
        let kept: String = s
            .chars()
            .take(LISTING_DESCRIPTION_CHARS.saturating_sub(1))
            .collect();
        format!("{}…", kept.trim_end())
    }

    /// `/skills` with no argument: list every skill in the already
    /// populated `index` — no fresh `discover()` call. The index is
    /// (re)built only by `HostStarting` (once, at startup) and
    /// `/reload-skills`; this mirrors `internal:user-agents`, whose own
    /// commands never rediscover on an unrelated invocation either.
    async fn list_skills(&self) -> Vec<Effect> {
        let all = self.index.sorted_snapshot().await;
        let warning_note = if self.last_warning_count > 0 {
            Some(note_line(format!(
                "warning: skipped {} skill definition(s) due to parse errors or tier shadowing; \
                 see logs for details",
                self.last_warning_count
            )))
        } else {
            None
        };

        if all.is_empty() {
            let mut effects = vec![note_line("no skills discovered")];
            effects.extend(warning_note);
            return effects;
        }

        let mut effects = Vec::with_capacity(all.len() + 2);
        effects.push(note_line(format!(
            "skills: {} discovered (descriptions are untrusted repo text)",
            all.len()
        )));
        // Descriptions come from files otto did not author, so they stay
        // labelled as untrusted wherever they are rendered.
        for skill in &all {
            effects.push(note_line(format!(
                "- {} [{}] — [untrusted repo skill description] {} (source: {})",
                skill.name,
                skill.scope.label(),
                Self::truncate_for_listing(&skill.description),
                skill.source.display(),
            )));
        }
        effects.extend(warning_note);
        effects
    }

    /// `/skills <name>`: inject the skill's full body directly into the
    /// conversation, subject to the level-3 trust gate. This mirrors
    /// `internal:user-slash-commands`' own `handle_slash`, which submits
    /// an expanded command body via `Effect::PromptSend` — the payload
    /// actually reaches `Host`'s conversation history and triggers a
    /// turn, unlike `Effect::PushNote`, which only appends to the TUI's
    /// own display log. Unlike the `skill` tool (which returns an
    /// *error string* on a trust refusal, since the model must not be
    /// told to route around it), this is a user-initiated command, so a
    /// gated skill instead opens the trust modal — the one path the spec
    /// calls out where the interactive trust prompt is reachable for a
    /// skill.
    async fn show_skill(&mut self, name: &str) -> Result<Vec<Effect>, PluginError> {
        let Some(spec) = self.index.get(name).await else {
            let known = self.index.names_snapshot().await;
            return Ok(vec![note_line(skill_tool::unknown_skill_message(
                name, &known,
            ))]);
        };

        match crate::plugin::builtin::user_skills::trust::evaluate(
            &spec,
            &self.project_root,
            &self.trust_levels,
        )
        .await
        {
            SkillTrust::Allowed => Ok(vec![Effect::PromptSend {
                text: skill_tool::render_skill(
                    &spec.name,
                    &spec.root.display().to_string(),
                    &spec.body,
                ),
            }]),
            SkillTrust::NeedsConsent { .. } => Ok(vec![
                // Stash so the trust modal can re-run `/skills <name>`
                // once the user decides — same sequence
                // `internal:user-slash-commands` uses for a gated
                // project command.
                Effect::StashPendingSlash {
                    name: "skills".into(),
                    args: vec![name.to_string()],
                },
                Effect::OpenScreen {
                    id: "trust.modal".into(),
                    args: ScreenArgs::TrustModal {
                        project_root: self.project_root.clone(),
                    },
                },
            ]),
        }
    }
}

impl Default for UserSkillsPlugin {
    fn default() -> Self {
        Self::new()
    }
}

fn note_line(text: impl Into<String>) -> Effect {
    Effect::PushNote {
        line: StyledLine::plain(text.into()),
    }
}

#[async_trait]
impl Plugin for UserSkillsPlugin {
    fn manifest(&self) -> Manifest {
        let mut contributions = Contributions::default();
        contributions.slash_commands = vec![
            SlashSpec {
                name: "skills".into(),
                summary: "List discovered skills, or load one by name".into(),
                args_hint: Some("[name]".into()),
                requires_arg: false,
                suppress_prompt_segments: vec![],
            },
            SlashSpec {
                name: "reload-skills".into(),
                summary: "Rescan discovered skills".into(),
                args_hint: None,
                requires_arg: false,
                suppress_prompt_segments: vec![],
            },
        ];
        contributions.hooks = vec![HookKind::HostStarting];
        // Read synchronously: `manifest()` cannot `.await`, which is
        // exactly why `catalog_cache` exists as a `std::sync::RwLock`
        // alongside the async `SkillIndex`. `None` (zero skills) omits
        // the segment entirely — spec acceptance criterion 3.
        let catalog = match self.catalog_cache.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        if let Some(text) = catalog {
            contributions.prompt_segments = vec![SystemPromptSegment {
                id: CATALOG_SEGMENT_ID.into(),
                text,
            }];
        }
        Manifest {
            id: PluginId::new("internal:user-skills").expect("valid built-in id"),
            name: "User skills".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Skills from .otto/skills/ and .claude/skills/".into(),
            kind: PluginKind::Core,
            contributions,
        }
    }

    async fn handle_slash(
        &mut self,
        name: &str,
        args: Vec<String>,
    ) -> Result<Vec<Effect>, PluginError> {
        match name {
            "skills" => match args.first() {
                None => Ok(self.list_skills().await),
                Some(skill_name) => self.show_skill(skill_name).await,
            },
            "reload-skills" => {
                let count = self.rediscover().await;
                let mut effects = self.register_skill_tool_effects().await;
                // A running session's system prompt only reflects
                // `Host::set_prompt_segments` calls it has already
                // received; without re-emitting this, `/reload-skills`
                // would update `catalog_cache` but a turn already in
                // flight (or started right after) would never see it.
                effects.push(Effect::ReloadPromptSegments);
                effects.push(note_line(format!(
                    "user-skills: reloaded ({count} skill(s))"
                )));
                Ok(effects)
            }
            _ => Ok(vec![]),
        }
    }

    async fn on_event(&mut self, event: HostEvent) -> Result<Vec<Effect>, PluginError> {
        if matches!(event, HostEvent::HostStarting) {
            self.rediscover().await;
            let mut effects = self.register_skill_tool_effects().await;
            // `main.rs` takes its one-shot startup snapshot of every
            // plugin's prompt segments *before* dispatching `HostStarting`
            // (so an eagerly-static segment like html-canvas's is present
            // for the very first turn) — which means it runs before
            // `rediscover()` above has populated `catalog_cache`. Without
            // this effect, a project with skills would show no level-1
            // catalog until the user manually ran `/reload-skills`. See
            // `crates/otto/src/main.rs`'s startup sequence, which drains
            // this alongside `apply_pending_in_process_tools` right after
            // dispatching this same event.
            effects.push(Effect::ReloadPromptSegments);
            Ok(effects)
        } else {
            Ok(vec![])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::manifests::Indexes;
    use crate::plugin::registry::PluginRegistry;
    use crate::plugin::slash::SlashRouter;
    use std::fs;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::RwLock;

    fn write_skill(root: &std::path::Path, location_dir: &str, slug: &str, body: &str) {
        let dir = root.join(location_dir).join(slug);
        fs::create_dir_all(&dir).expect("create skill dir");
        fs::write(dir.join("SKILL.md"), body).expect("write skill");
    }

    /// An empty shared trust map — no project has been decided yet.
    /// Mirrors `user_slash_commands::mod.rs` tests' own `empty_trust()`.
    fn empty_trust() -> crate::plugin::builtin::user_slash_commands::TrustMap {
        Arc::new(RwLock::new(std::collections::BTreeMap::new()))
    }

    fn note_text(effect: &Effect) -> String {
        match effect {
            Effect::PushNote { line } => line.spans.iter().map(|span| span.text.clone()).collect(),
            other => panic!("expected PushNote, got {other:?}"),
        }
    }

    #[test]
    fn manifest_contributes_skills_slash() {
        let plugin = UserSkillsPlugin::default();
        let manifest = plugin.manifest();
        assert_eq!(manifest.id.as_str(), "internal:user-skills");
        assert!(
            manifest
                .contributions
                .slash_commands
                .iter()
                .any(|slash| slash.name == "skills")
        );
    }

    #[test]
    fn manifest_subscribes_to_host_starting() {
        let plugin = UserSkillsPlugin::default();
        let manifest = plugin.manifest();
        assert!(
            manifest
                .contributions
                .hooks
                .contains(&HookKind::HostStarting)
        );
    }

    /// Acceptance criterion 3: a user with zero skills sees no skills
    /// segment in the system prompt at all, not an empty one.
    #[tokio::test]
    async fn manifest_omits_catalog_segment_when_no_skills() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        let mut plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            empty_trust(),
        );
        plugin
            .on_event(HostEvent::HostStarting)
            .await
            .expect("on_event");

        let manifest = plugin.manifest();

        assert!(
            !manifest
                .contributions
                .prompt_segments
                .iter()
                .any(|s| s.id == CATALOG_SEGMENT_ID),
            "zero skills must contribute no catalog segment at all"
        );
    }

    #[tokio::test]
    async fn manifest_includes_catalog_segment_when_skills_present() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        write_skill(
            project.path(),
            ".otto/skills",
            "otto-development",
            "---\nname: otto-development\ndescription: Build Otto changes\n---\nBody",
        );
        let mut plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            empty_trust(),
        );
        plugin
            .on_event(HostEvent::HostStarting)
            .await
            .expect("on_event");

        let manifest = plugin.manifest();

        let segment = manifest
            .contributions
            .prompt_segments
            .iter()
            .find(|s| s.id == CATALOG_SEGMENT_ID)
            .expect("catalog segment must be present once a skill is discovered");
        assert!(segment.text.contains("otto-development"));
    }

    #[tokio::test]
    async fn handle_slash_empty_state_reports_no_skills_discovered() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        let mut plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            empty_trust(),
        );

        let effects = plugin.handle_slash("skills", vec![]).await.expect("slash");

        assert_eq!(effects.len(), 1);
        assert_eq!(note_text(&effects[0]), "no skills discovered");
    }

    #[tokio::test]
    async fn invalid_skills_surface_a_warning_note() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        // Missing `description:` — the one frontmatter field that is a hard
        // parse error, because it is all the model sees at level 1.
        write_skill(
            project.path(),
            ".otto/skills",
            "broken",
            "---\nname: broken\n---\nBody",
        );
        let mut plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            empty_trust(),
        );
        // `/skills` (no arg) reads the already-populated index rather than
        // re-running discovery, so the index must be populated first —
        // exactly as `HostStarting` does at real startup.
        plugin
            .on_event(HostEvent::HostStarting)
            .await
            .expect("on_event");

        let effects = plugin.handle_slash("skills", vec![]).await.expect("slash");

        let lines: Vec<_> = effects.iter().map(note_text).collect();
        assert_eq!(lines[0], "no skills discovered");
        assert_eq!(
            lines[1],
            "warning: skipped 1 skill definition(s) due to parse errors or tier shadowing; see logs for details"
        );
    }

    #[tokio::test]
    async fn slash_output_includes_name_scope_description_and_source() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        write_skill(
            project.path(),
            ".otto/skills",
            "otto-development",
            "---\nname: otto-development\ndescription: Build Otto changes\n---\nBody",
        );
        write_skill(
            project.path(),
            ".claude/skills",
            "rust-engineer",
            "---\nname: rust-engineer\ndescription: Build Rust systems\n---\nBody",
        );

        let mut plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            empty_trust(),
        );
        plugin
            .on_event(HostEvent::HostStarting)
            .await
            .expect("on_event");
        let registry = PluginRegistry::from_plugins(vec![Box::new(plugin)]);
        let indexes = Indexes::build(&registry).await.expect("indexes");
        let router = SlashRouter::new(
            Arc::new(RwLock::new(indexes)),
            Arc::new(RwLock::new(registry)),
        );

        let effects = router.dispatch("skills", vec![]).await.expect("dispatch");
        let lines: Vec<_> = effects.iter().map(note_text).collect();

        assert_eq!(
            lines[0],
            "skills: 2 discovered (descriptions are untrusted repo text)"
        );
        assert!(lines[1].contains("otto-development"));
        assert!(lines[1].contains("[project/.otto]"));
        assert!(lines[1].contains("[untrusted repo skill description]"));
        assert!(lines[1].contains("Build Otto changes"));
        assert!(
            lines[1].contains("source:")
                && lines[1].contains("otto-development")
                && lines[1].contains("SKILL.md"),
            "listing must show the source path, got: {}",
            lines[1]
        );
        assert!(lines[2].contains("rust-engineer"));
        assert!(lines[2].contains("[project/.claude]"));
        assert!(lines[2].contains("[untrusted repo skill description]"));
        assert!(lines[2].contains("Build Rust systems"));
    }

    /// The four tiers share a slug namespace: the highest-precedence copy
    /// wins and the loser is reported, so an author editing the shadowed
    /// file learns why nothing changed.
    #[tokio::test]
    async fn project_tier_shadows_user_tier_and_warns() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        write_skill(
            project.path(),
            ".otto/skills",
            "shared",
            "---\nname: shared\ndescription: Project copy\n---\nBody",
        );
        write_skill(
            home.path(),
            ".claude/skills",
            "shared",
            "---\nname: shared\ndescription: User copy\n---\nBody",
        );

        let mut plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            empty_trust(),
        );
        plugin
            .on_event(HostEvent::HostStarting)
            .await
            .expect("on_event");
        let effects = plugin.handle_slash("skills", vec![]).await.expect("slash");
        let lines: Vec<_> = effects.iter().map(note_text).collect();

        assert_eq!(
            lines[0],
            "skills: 1 discovered (descriptions are untrusted repo text)"
        );
        assert!(lines[1].contains("Project copy"));
        assert_eq!(
            lines[2],
            "warning: skipped 1 skill definition(s) due to parse errors or tier shadowing; see logs for details"
        );
    }

    /// `/skills` (no arg) must read the already-populated index rather
    /// than re-running `discover()`. A skill written to disk *after*
    /// `HostStarting` must not appear until `/reload-skills` runs.
    #[tokio::test]
    async fn skills_no_arg_does_not_rediscover() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        write_skill(
            project.path(),
            ".otto/skills",
            "first",
            "---\nname: first\ndescription: First skill\n---\nBody",
        );
        let mut plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            empty_trust(),
        );
        plugin
            .on_event(HostEvent::HostStarting)
            .await
            .expect("on_event");

        // Written after the index was built; `/skills` must not see it.
        write_skill(
            project.path(),
            ".otto/skills",
            "second",
            "---\nname: second\ndescription: Second skill\n---\nBody",
        );

        let effects = plugin.handle_slash("skills", vec![]).await.expect("slash");
        let lines: Vec<_> = effects.iter().map(note_text).collect();
        assert_eq!(
            lines[0], "skills: 1 discovered (descriptions are untrusted repo text)",
            "must reflect the index as of the last discovery, not disk right now"
        );
        assert!(lines[1].contains("first"));
        assert!(
            !lines.iter().any(|l| l.contains("second")),
            "a skill added after HostStarting must not appear until /reload-skills"
        );
    }

    /// Extracts the text of a `PromptSend` effect, panicking on any other
    /// effect shape. `/skills <name>` must submit the body to the
    /// provider as if the user had typed it — `Effect::PromptSend` is
    /// the only effect that reaches `Host`'s conversation history and
    /// triggers a turn; `Effect::PushNote` only appends to the TUI's own
    /// display log and is never seen by the model.
    fn prompt_text(effect: &Effect) -> String {
        match effect {
            Effect::PromptSend { text } => text.clone(),
            other => panic!("expected PromptSend, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn skills_with_name_sends_body_and_root_as_a_prompt_for_an_allowed_skill() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        write_skill(
            project.path(),
            ".otto/skills",
            "otto-development",
            "---\nname: otto-development\ndescription: Build Otto changes\n---\nFull instructions here",
        );
        let mut plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            empty_trust(),
        );
        plugin
            .on_event(HostEvent::HostStarting)
            .await
            .expect("on_event");

        let effects = plugin
            .handle_slash("skills", vec!["otto-development".into()])
            .await
            .expect("slash");

        assert_eq!(effects.len(), 1);
        assert!(
            matches!(&effects[0], Effect::PromptSend { .. }),
            "the skill body must be sent as a prompt so the model sees it, not pushed as a \
             display-only note: {:?}",
            effects[0]
        );
        let text = prompt_text(&effects[0]);
        assert!(
            text.contains("Full instructions here"),
            "body must be injected, got: {text}"
        );
        assert!(
            text.contains(".otto/skills/otto-development"),
            "skill root must be stated, got: {text}"
        );
    }

    #[tokio::test]
    async fn skills_with_name_opens_trust_modal_for_gated_untrusted_skill() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        write_skill(
            project.path(),
            ".otto/skills",
            "deployer",
            "---\nname: deployer\ndescription: Deploys things\n---\nrun scripts/deploy.sh",
        );
        // Bundled executable so the level-3 trust gate applies.
        let scripts_dir = project.path().join(".otto/skills/deployer/scripts");
        fs::create_dir_all(&scripts_dir).expect("mkdir scripts");
        fs::write(scripts_dir.join("deploy.sh"), "#!/bin/sh\necho hi").expect("write script");

        let mut plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            empty_trust(),
        );
        plugin
            .on_event(HostEvent::HostStarting)
            .await
            .expect("on_event");

        let effects = plugin
            .handle_slash("skills", vec!["deployer".into()])
            .await
            .expect("slash");

        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::PushNote { line } if line.spans.iter().any(|s| s.text.contains("run scripts/deploy.sh")))),
            "the body must not leak into any effect before trust is granted: {effects:?}"
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::OpenScreen { id, .. } if id == "trust.modal")),
            "expected OpenScreen(trust.modal), got: {effects:?}"
        );
        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::StashPendingSlash { name, args }
                    if name == "skills" && args == &vec!["deployer".to_string()]
            )),
            "expected StashPendingSlash(skills, [deployer]), got: {effects:?}"
        );
    }

    /// Regression test for the trust-gate bug: once the user picks
    /// "session-text-only" in the trust modal, `/skills <name>` for the
    /// same gated skill must succeed on the very next call — not reopen
    /// the modal again — and nothing needs to be written to disk for
    /// that to work. Before the fix, `trust::evaluate` re-read
    /// `~/.otto/trusted-projects.json` on every call, which never
    /// reflects a `SessionTextOnly` decision (deliberately never
    /// persisted), so the modal reopened every time.
    #[tokio::test]
    async fn skills_with_name_honors_session_text_only_decision_without_reopening_modal() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        write_skill(
            project.path(),
            ".otto/skills",
            "deployer",
            "---\nname: deployer\ndescription: Deploys things\n---\nrun scripts/deploy.sh",
        );
        let scripts_dir = project.path().join(".otto/skills/deployer/scripts");
        fs::create_dir_all(&scripts_dir).expect("mkdir scripts");
        fs::write(scripts_dir.join("deploy.sh"), "#!/bin/sh\necho hi").expect("write script");

        let trust = empty_trust();
        let mut plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            trust.clone(),
        );
        plugin
            .on_event(HostEvent::HostStarting)
            .await
            .expect("on_event");

        // First call: no decision yet, gated.
        let first = plugin
            .handle_slash("skills", vec!["deployer".into()])
            .await
            .expect("slash");
        assert!(
            first
                .iter()
                .any(|e| matches!(e, Effect::OpenScreen { id, .. } if id == "trust.modal")),
            "expected the modal to open on the first call, got: {first:?}"
        );

        // Simulate the user choosing "session-text-only" in the trust
        // modal, exactly as `Effect::SetTrustLevel`'s handler would:
        // insert into the shared map directly, no disk write.
        trust.write().await.insert(
            project.path().to_path_buf(),
            crate::plugin::builtin::user_slash_commands::trust::TrustLevel::SessionTextOnly,
        );

        // Second call, same session: must succeed as a PromptSend, not
        // reopen the modal.
        let second = plugin
            .handle_slash("skills", vec!["deployer".into()])
            .await
            .expect("slash");
        assert!(
            !second
                .iter()
                .any(|e| matches!(e, Effect::OpenScreen { .. })),
            "the modal must not reopen once session-text-only is granted, got: {second:?}"
        );
        assert_eq!(second.len(), 1);
        let text = prompt_text(&second[0]);
        assert!(
            text.contains("run scripts/deploy.sh"),
            "the body must now be sent, got: {text}"
        );
    }

    #[tokio::test]
    async fn skills_with_unknown_name_reports_near_matches() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        write_skill(
            project.path(),
            ".otto/skills",
            "rust-engineer",
            "---\nname: rust-engineer\ndescription: Build Rust systems\n---\nBody",
        );
        let mut plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            empty_trust(),
        );
        plugin
            .on_event(HostEvent::HostStarting)
            .await
            .expect("on_event");

        let effects = plugin
            .handle_slash("skills", vec!["rust-enginer".into()])
            .await
            .expect("slash");

        assert_eq!(effects.len(), 1);
        let text = note_text(&effects[0]);
        assert!(text.contains("unknown skill"), "got: {text}");
        assert!(text.contains("rust-engineer"), "got: {text}");
    }

    #[tokio::test]
    async fn reload_skills_reregisters_tool_reloads_prompt_and_reports_count() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        write_skill(
            project.path(),
            ".otto/skills",
            "otto-development",
            "---\nname: otto-development\ndescription: Build Otto changes\n---\nBody",
        );
        let mut plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            empty_trust(),
        );

        let effects = plugin
            .handle_slash("reload-skills", vec![])
            .await
            .expect("slash");

        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::RegisterInProcessTool { .. })),
            "expected RegisterInProcessTool, got: {effects:?}"
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ReloadPromptSegments)),
            "expected ReloadPromptSegments, got: {effects:?}"
        );
        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::PushNote { line }
                    if line.spans.iter().any(|s| s.text.contains("reloaded") && s.text.contains('1'))
            )),
            "expected a count-reporting PushNote, got: {effects:?}"
        );
    }

    #[tokio::test]
    async fn skill_tool_not_registered_when_index_empty() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        let plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            empty_trust(),
        );

        let effects = plugin.register_skill_tool_effects().await;

        assert!(
            effects.is_empty(),
            "no skill tool until at least one skill discovered"
        );
    }

    #[tokio::test]
    async fn skill_tool_registered_when_index_has_skills() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        write_skill(
            project.path(),
            ".otto/skills",
            "otto-development",
            "---\nname: otto-development\ndescription: Build Otto changes\n---\nBody",
        );
        let mut plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            empty_trust(),
        );

        let effects = plugin
            .on_event(HostEvent::HostStarting)
            .await
            .expect("on_event");

        assert_eq!(effects.len(), 2);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::RegisterInProcessTool { .. })),
            "expected RegisterInProcessTool, got {effects:?}"
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ReloadPromptSegments)),
            "expected ReloadPromptSegments so a live session's system prompt picks up the \
             catalog discovery just did, got {effects:?}"
        );
    }

    /// Regression test for a review finding on PR #153: `main.rs` takes its
    /// one-shot startup snapshot of prompt segments *before* dispatching
    /// `HostStarting`, so without this effect a project with skills would
    /// show no level-1 catalog until the user manually ran `/reload-skills`.
    /// This must hold even with zero skills discovered, so the *first*
    /// `HostStarting` after a skill is later added still has a path to
    /// getting its segment live (via a subsequent `/reload-skills`, which
    /// reuses this same effect).
    #[tokio::test]
    async fn host_starting_always_emits_reload_prompt_segments() {
        let project = tempdir().expect("tempdir");
        let home = tempdir().expect("tempdir");
        let mut plugin = UserSkillsPlugin::with_roots(
            project.path().to_path_buf(),
            home.path().to_path_buf(),
            empty_trust(),
        );

        let effects = plugin
            .on_event(HostEvent::HostStarting)
            .await
            .expect("on_event");

        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ReloadPromptSegments)),
            "even with zero skills, HostStarting must emit ReloadPromptSegments so the \
             (empty) catalog state is reflected rather than left at whatever main.rs's \
             pre-HostStarting snapshot happened to capture: {effects:?}"
        );
    }
}
