# Changelog

All notable changes to otto are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: `0.MINOR.PATCH`, where MINOR captures features + breaking
boundary changes and PATCH captures fixes).

## [Unreleased]

## 0.30.8 - 2026-09-14

### Added

- A regression test proving the `changelog` screen's own content survives its `tips()` row when
  painted through its `CenteredModal` layout, closing the last open acceptance-criterion gap in
  `savvagent/otto#119` (the other two — `paint_screen` reserving the tips row for
  `Fullscreen`/`BottomSheet` layouts, and `command_palette`'s hardcoded budget reduction — were
  already shipped in 0.30.6/0.30.7 via #122). Test-only; no production code or public-interface
  change. (#119, #175)

## 0.30.7 - 2026-09-11

### Changed

- `plugin-wasm-parity-policy` record-as-shipped update: the design spec's `Status:` is flipped to
  `IMPLEMENTED` and the plan's remaining checkboxes are ticked for the plugin-ABI native-vs-WASM
  parity policy shipped in 0.30.6 (no functional change). (#173)

## 0.30.6 - 2026-09-11

### Added

- `CLAUDE.md` now records a position on the native-vs-WASM plugin ABI: parity is the goal for the
  plugin ABI's guest-callable behavioral surface and for constructor ergonomics the guest side can
  express without a WIT change, and enumerates the current gap — `Screen::ghost_completion` (#165),
  the `StyledSpan`/`StyledLine` constructor ergonomics (#166), the entirely-unbridged
  `ContentRenderer`/canvas surface (#167), and `Plugin::summarize_tool_call`/`summarize_tool_result`
  plus `Contributions::tool_summaries` (#170) — each tracked by its own issue instead of a bare "for
  now." A future changelog entry citing a WASM gap should reference an issue number the same way.
  `otto-plugin`/`otto-plugin-wit`/`otto-plugin-wasm` are also added to `CLAUDE.md`'s Workspace map
  table, previously absent. Documentation-only; no Rust, `.wit`, or runtime behavior changed.
  (#140)

### Changed

- `tool-stderr-inherit-fix` record-as-shipped update: the plan's remaining checkboxes are ticked
  and the design spec's `Status:` is flipped to `IMPLEMENTED` for the fix already released in
  0.30.5 (no functional change). (#171)

## 0.30.5 - 2026-09-11

### Fixed

- Stdio tool child processes (`tool-fs`, `tool-web`, `tool-bash`, etc.) no longer inherit the TUI's
  terminal stderr. `rmcp`'s `TokioChildProcess::new(cmd)` convenience constructor silently
  re-applies its own default `Stdio::inherit()` for stderr at spawn time, discarding whatever
  otto's own stderr redirection had already configured on the `Command` — a pre-existing defect in
  every stdio tool spawn that could corrupt the TUI's rendered screen the first time a tool server
  was spawned after the terminal entered raw/alternate-screen mode, most visibly on a mid-session
  `/connect` that builds a fresh host on demand (e.g. connecting to DeepSeek). All three spawn sites
  now route through a dedicated helper that honors the caller's stderr `Stdio` explicitly. (#146)

### Changed

- `otto-development`'s Non-Negotiable Rule 8 documentation record-as-shipped update (no functional
  change). (#164)

## 0.30.4 - 2026-09-11

### Changed

- `otto-development`'s Non-Negotiable Rule 8 no longer says a release is "not optional and not
  batchable across PRs" — it now permits batching already-merged, unreleased work into one release
  at cut time (matching standing, owner-approved practice), states that a batched release's line
  is the highest SemVer bump required across the batch, and requires the release PR body to
  enumerate every issue/PR the release covers. The plan format's `Release line:` field is reframed
  as a floor rather than a predicted version. Canonical and Claude Code port skill docs updated
  together, per the repo's skill-porting convention. (#138)

## 0.30.3 - 2026-09-11

### Fixed

- `ci.yml`'s concurrency group no longer cancels the wrong run: `cancel-in-progress` is now a pure
  function of `github.ref` instead of `github.event_name`, so it can no longer disagree across two
  runs sharing a group, and a push to `main`/`master` is now structurally guaranteed non-cancellable
  by the group rather than only incidentally so. The group key moved to the standard
  `github.head_ref || github.ref` formulation. Every CI job now has an explicit `timeout-minutes`
  ceiling (with a longer allowance for the Windows leg of the test matrix) so a wedged run can no
  longer starve subsequent pushes to the same PR indefinitely. (#139)

## 0.30.2 - 2026-09-11

### Added

- otto now discovers Claude-Code-compatible skills — `SKILL.md` files under `.otto/skills/` and
  `.claude/skills/` in both project and user scope, plus `.github/skills/` at project scope only
  (Copilot CLI's location; it has no user-scope counterpart) — and exposes them through
  progressive disclosure: a name+description catalog is folded into the system prompt
  (omitted entirely when no skills are found), and the model can load a skill's full
  instructions on demand by calling the new built-in `skill` tool. A project-scope skill that
  bundles scripts still requires the same per-project trust consent as project slash commands
  before its body is returned. (#83)
- `/skills` lists every discovered skill (name, source, scope, description) from the
  already-discovered index rather than rescanning disk on every invocation, and `/skills <name>`
  injects that skill's full instructions directly into the conversation without waiting for the
  model to call the `skill` tool — opening the same trust-confirmation modal project slash
  commands use for a gated, not-yet-trusted skill, and resuming `/skills <name>` automatically
  once the user decides. A new `/reload-skills` command rescans all five tiers, re-registers the
  `skill` tool against the refreshed set, and pushes the updated catalog into a running session's
  system prompt — previously that catalog was only ever set once, at startup. (#83)

## 0.30.1 - 2026-09-11

### Fixed

- The `otto-development` Claude Code skill port's Code Quality Review and Final Code Review dispatch
  templates now carry an explicit read-only, high-confidence-only constraint, restoring the
  independence guarantee the port's own text already claimed for them (mirroring the treatment
  already used for the security-review template). Previously, a `general-purpose` subagent dispatched
  for either review retained its full write toolset with no textual constraint, contradicting three
  places in the skill that asserted otherwise. (#137)
- Corrected several smaller documentation drifts in the same skill body: a `TodoWrite` status
  vocabulary mismatch (`done` vs. the port's own `completed`), an inaccurate claim about `/clear`
  availability in Claude Code, a diff-delivery contradiction in the security-review prompt's prose,
  and two stale "ignored by design" claims in the skill-porting design spec that predated the shipped
  `NATIVE_SKILLS`-declaration check. (#137)
- `check-claude-skill-ports.sh` now increments its `checked` counter only after a file survives the
  symlink and missing-port rejection checks, correcting the bookkeeping order. (#137)

## 0.30.0 - 2026-09-11

### Added

- xAI Grok is now a sixth built-in provider (`crates/provider-grok`), selectable from `/connect`,
  `/model`, `/use`, and the `@provider:model` routing prefix. Targets xAI's OpenAI-compatible Chat
  Completions endpoint, with a `grok-4` (default) and `grok-4-fast` model catalog, full SSE
  streaming, and a standalone `otto-grok` binary alongside the existing provider MCP servers. (#103)

## 0.29.1 - 2026-09-11

### Fixed

- The `otto-development` skill no longer instructs a release run to verify `.deb`/`.rpm` packages
  or a `package-linux.yml` workflow run, both of which were deliberately removed from the release
  pipeline in an earlier change. The skill's Phase 5 release verification now checks against the
  actual 14-asset `cargo-dist` release set instead, matching what `RELEASING.md` has always said.
  Previously, an agent following the skill as written would escalate on every otherwise-successful
  release. (#141)

## 0.29.0 - 2026-09-10

### Added

- The `/` command palette prompt now shows the remainder of the highlighted command as dim,
  non-editable "ghost" text immediately after the cursor, whenever that command's name is a
  literal prefix completion of what's typed (e.g. typing `/co` toward `connect` shows the `nnect`
  remainder dimmed). Restores the predictive signal the prompt lost when #96 made it echo raw
  keystrokes instead of the resolved command name — without reintroducing #96's defect, since the
  ghost text is never part of the prompt's real, submittable content. When the highlighted row only
  substring-matches what's typed (not a prefix), no ghost text renders and the existing `tips()`
  row remains the only disambiguation signal, unchanged. Adds `Screen::ghost_completion` as a new,
  additive, default-`None` method on the plugin `Screen` trait (`crates/otto-plugin`) for native
  screen authors who want the same overlay on their own screens — not yet exposed through the WASM
  plugin ABI's WIT interface, so third-party WASM plugins inherit the default (no ghost text) for
  now. (#118)

- This repo's own `otto-development` and `creating-github-issues` workflow skills are now
  discoverable by Claude Code, as adapted ports under `.claude/skills/` alongside the existing
  `rust-engineer` and `tui-engineer`. Claude Code reads project skills from `.claude/skills/` only,
  so both were previously invisible to it while living solely in `.github/skills/` in the Copilot
  CLI's format. They are ports rather than symlinks or verbatim copies because the canonical bodies
  name Copilot-specific dispatch mechanics at every step; the adaptation touches only those
  host-mechanism references, leaving every Non-Negotiable Rule, phase gate, fix-loop cap and repo
  convention byte-identical — about 8% of the canonical text, recorded exactly as a committed
  `diff -u` per ported file. A new `.github/scripts/check-claude-skill-ports.sh`, run by CI's `lint`
  job, recomputes each diff and fails the build if the two copies drift, if a port or record is
  missing or orphaned, or if a port's `name`/`description` frontmatter diverges from its canonical.
  Contributor-facing only — no runtime behaviour change. (#128)

### Fixed

- DeepSeek's `/connect` no longer swallows a rejected (or later, corrected) API key when no host
  is running yet. `apply_pending_pool_add` — the drain that runs after the provider picker's
  silent-connect path — used to bail out with only a `tracing::warn!` whenever `current_host` was
  `None`, before it ever re-validated the stored credential, so the rejection vanished with no
  note, no host, and no way to retry. This is reachable for any keyed provider, but hit DeepSeek in
  practice since it's commonly the first provider connected in a session with no host already up.
  `apply_pending_pool_add` now re-validates the credential regardless of whether a host exists,
  always surfaces the outcome as a note, and builds a fresh host on demand on success — sharing
  that bootstrap path with `perform_connect`'s existing first-connect branch. (#81)

## 0.28.1 - 2026-09-10

### Fixed

- `Screen::render`'s `Fullscreen` and `BottomSheet` layouts no longer let the runtime overpaint a
  screen's own last row with `tips()`. The runtime now reserves that row out of the region handed
  to `render` before calling it, instead of painting `tips()` over whatever the screen already drew
  there — matching `Screen::render`'s documented contract that chrome is painted around the
  screen's content, not inside it. The command palette's own reserved-row budget shrinks
  accordingly, since it no longer needs to compensate for the overpaint itself. (#116)

## 0.28.0 - 2026-09-10

### Added

- `otto-plugin` gains constructors for the styled-text shape plugin screens build most often:
  `StyledSpan::plain`, `StyledSpan::colored`, `StyledSpan::muted`, `StyledLine::colored` and
  `StyledLine::muted`, alongside the existing `StyledLine::plain`. Each sets no background and no
  text modifiers; anything richer still builds the struct literal, which is unchanged and remains
  public. Every builtin screen and tool-summary plugin now goes through these constructors, which
  also removes eight file-local copies of the same two-line function that had accumulated across
  the tree. Nothing renders differently.

  This is additive and affects in-tree Rust consumers of `otto-plugin` only. WASM plugin authors
  are unchanged either way: they bind against `otto-plugin-wit`, where `styled-span` / `styled-line`
  are WIT `record`s that cannot carry associated functions at all, and they build the generated
  `wit::StyledSpan` — a different type, with a `Reset` sentinel rather than an `Option` for `fg`.
  Guest-side ergonomics remain an open gap. (#117)

## 0.27.1 - 2026-09-10

### Fixed

- While the `/` command palette is open, the prompt input now echoes what you typed rather than
  the command currently highlighted in the list. Typing `/co` leaves `/co` in the prompt instead
  of replacing it with `/connect`, arrow keys move the selection without rewriting the prompt, and
  opening the palette seeds a bare `/` instead of the first command in the list. The resolved
  command reaches the prompt only when you select it, which is unchanged. Backspacing past the
  leading `/` now closes the palette, since that character is no longer something the palette owns
  on your behalf. The palette's own `> <filter>` header is gone — it duplicated the prompt one row
  below it — and a filter matching no commands now says so instead of leaving the list blank.
  The palette's tips row now names the command Enter would run, so the pending command is still
  stated somewhere: command matching is a substring match over every enabled plugin's slash
  commands, so the highlighted row is not always the command you are partway through typing.
  Reverses the prompt-mirroring behavior added in 0.26.4. (#96)

## 0.27.0 - 2026-09-09

### Added

- Otto now ships a built-in `/skills` slash command that lists skills discovered from
  `.otto/skills/*/SKILL.md` and `.claude/skills/*/SKILL.md` at both project and user scope, plus
  the project's `.github/skills/*/SKILL.md`, including each skill's name, source tier, and
  file-supplied description, with that description explicitly labeled as untrusted text. Skills
  that fail to parse are skipped and reported as a count rather than silently omitted. (#83)

### Changed

- **Breaking: the provider picker is the only way to connect.** `/connect` now ignores any
  argument and always opens the picker; the provider-specific `/connect <provider>` commands
  (`/connect anthropic`, `/connect gemini`, and the rest) are gone from the typed-command surface
  and from the `/` command palette. Re-keying moved onto the picker too — highlight a provider and
  press <kbd>Alt</kbd>+<kbd>Enter</kbd> instead of passing `--rekey`. The `connect <id>` slashes
  still exist as internal plumbing for the picker, silent stored-key reconnect, and `--rekey`, so
  no plugin ABI, SPP wire format, or on-disk format changed. Removing user-facing commands is
  breaking, which ships this as a MINOR release under this repo's pre-1.0 SemVer policy. Recovery
  hints across all four locale catalogs now point at `/connect` and the picker. (#82)

### Removed

- The `.deb` and `.rpm` Linux packages, along with the `package-linux.yml` workflow and the
  `cargo-deb` / `cargo-generate-rpm` metadata that fed it. Linux installs go through the shell
  installer or the platform tarball, which is what the README has always documented; the distro
  packages were never referenced there. The two packaging tools share no configuration, so the
  build carried two hand-synced asset lists enumerating all ten binaries. Releases published
  before this change keep the assets they already have. (#108)

- The experimental `eframe`/`egui` native GUI front-end and its `otto gui` entry point, along with
  the `eframe`, `egui`, and `egui-file-dialog` dependencies. `otto gui` no longer launches a
  window — the `gui` argument is simply ignored and the TUI launches as usual; the ratatui TUI is
  unchanged and is now otto's only front-end. The removed code stays recoverable from git history.
  (#94)

## 0.26.4 - 2026-09-08

### Added

- The inline `/` command palette now mirrors the currently-highlighted slash command into the
  prompt input: pressing `/` over an empty prompt opens the palette and immediately seeds the
  prompt with the first matching command, typing and arrow-navigation keep the prompt in sync with
  the highlighted row (falling back to `/<filter>` when nothing matches), and closing or running a
  command clears the staged text. The palette only ever opens over an empty prompt, so an
  in-progress draft is never overwritten. Both the ratatui TUI and the egui front-end are covered.
  (#80)

### Fixed

- The DeepSeek provider's `list_models` call (used by connect-time validation) now classifies
  non-2xx `/models` responses by HTTP status into the matching error kind instead of always
  reporting a network error, so a rejected or expired key surfaces the same `--rekey` recovery
  path as turn-time authentication failures. (#89)

## 0.26.3 - 2026-09-08

### Fixed

- `otto-plugin-wasm`'s `wasmtime::component::bindgen!` macros no longer reference a sibling
  `../otto-plugin-wit/wit` path, which made the crate fail to compile whenever Cargo verified it
  in isolation (e.g. `cargo package --workspace`, as run by the `release-plz` automation). The
  crate now vendors its own copy of the `.wit` files, kept in sync with `otto-plugin-wit`'s
  canonical copy via a build-script check during normal workspace builds. (#78)

## 0.26.2 - 2026-09-08

### Fixed

- Failure notes for a rejected/expired provider API key now point at the existing `/connect <id>
  --rekey` command instead of leaving a dead end: a connect-time rejection caused by a bad key
  (not a rate limit, permission/quota failure, or a keyless provider) gets a `--rekey` hint, and a
  turn-time authentication failure gets a companion note naming the actual routed provider (not
  just whatever provider happens to be "active"). Both the TUI and GUI front-ends are covered. (#81)

## 0.26.1 - 2026-09-08

### Fixed

- The startup splash and the `/splash` command now share the same rendering logic, so both
  surfaces stay visually consistent instead of drifting out of sync. (#66)

## 0.26.0 - 2026-09-08

### Added

- OAuth 2.1 authorization-code + PKCE support for remote Streamable HTTP MCP servers configured
  via `[[mcp_servers]]`, including dynamic client registration (DCR), keyring-backed token/state
  persistence under the existing `mcp:<server name>` namespace, and `/mcp` UX to authorize, check,
  and clear an OAuth authorization for a server. Discovery and persistence are hardened with
  endpoint pinning, same-origin protected-resource metadata checks, cross-origin private-host/DNS
  rejection, redirect-free DCR, sanitized error rendering, and stale/concurrent-flow guards. (#49)

## 0.25.4 - 2026-09-08

### Fixed

- Startup no longer trusts an equal-version update-check cache entry as authoritative — it always
  revalidates live before deciding Otto is up to date, so an available update is now surfaced
  reliably. (#65)
- `/update` now performs an authoritative live release check and serializes installer admission so
  overlapping checks can no longer race into duplicate or failed installs. (#65)

## 0.25.3 - 2026-09-07

### Fixed

- The startup splash screen's ASCII-art logo now spells "OTTO" instead of the old "SAVVAGENT"
  block-letter art. (#67)

## 0.25.2 - 2026-09-07

### Added

- `.github/skills/creating-github-issues/` — a repo-appropriate, GitHub-Issues-only ticket-creation
  skill (type label, duplicate check, no fabricated priority/estimate fields), replacing the need
  for the personal JIRA-default `creating-tickets` skill in this repo. (#55)

## 0.25.1 - 2026-09-07

### Fixed

- Startup provider auto-connect no longer pushes "build failed"/"timeout"/"rejected" notes into
  the transcript by default — only `tracing` log entries. A normal launch with a healthy key stays
  quiet; opt back into the previous chatter with `[startup] verbose = true` in `~/.otto/config.toml`.
- A present-but-rejected provider key (bad key, no billing credit, rate-limited, org disabled, ...)
  is now detected via `list_models` at connect time instead of being registered as a
  falsely-healthy provider. This required fixing `provider-gemini`/`provider-openai`'s `list_models`
  error classification, which previously mapped all HTTP error statuses to `ErrorKind::Network`,
  masking auth/permission/rate-limit errors as generic connectivity issues.
- `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`/`GOOGLE_API_KEY`, `OPENAI_API_KEY`, and `DEEPSEEK_API_KEY`
  are now recognized as a fallback credential source (in addition to the keyring) at startup and
  via `/connect`, not just when the provider server binaries run standalone.
- Turn-time provider failures now name the offending provider, e.g. `Error: Anthropic rejected the
  request: <message>` instead of the unattributed `Error: provider error: Authentication: <message>`.
- `provider-deepseek`'s shim (added in 0.25.0, before this fix) is updated to the same
  `ProviderBuildOutcome`-based registration flow as the other four built-in providers. (#14)

## 0.25.0 - 2026-09-07

### Added

- DeepSeek as a fifth built-in provider (`crates/provider-deepseek`), covering the OpenAI-compatible
  Chat Completions API at `https://api.deepseek.com` (`deepseek-v4-flash` default,
  `deepseek-v4-pro`), the `otto-deepseek` standalone MCP-server binary, and the
  `DEEPSEEK_API_KEY`/`DEEPSEEK_BASE_URL`/`OTTO_DEEPSEEK_LISTEN` environment variables. (#57)

## 0.24.0 - 2026-09-07

### Changed

- **Breaking, comprehensive rename: the project is renamed from `savvagent` to `otto`.** Every
  public interface is affected, with no compatibility shims or migration path: the crate prefix
  (`savvagent-*` → `otto-*`), the TUI binary and all bundled tool/provider binaries (`savvagent` →
  `otto`, `savvagent-tool-*` → `otto-tool-*`, `savvagent-anthropic`/`-gemini`/`-openai` →
  `otto-anthropic`/`-gemini`/`-openai`), the on-disk config/state directory (`~/.savvagent/` →
  `~/.otto/`), the project-context filename (`SAVVAGENT.md` → `OTTO.md`), the OS keyring service
  name (`savvagent` → `otto`), every `SAVVAGENT_*` environment variable (→ `OTTO_*`), the plugin
  WIT package (`savvagent:plugin@0.1.0` → `otto:plugin@0.1.0`), and the plugin manifest's
  `[plugin].savvagent` version-range key (→ `[plugin].otto`). The GitHub org handle `savvagent`
  is unchanged; only the repository name changed (`savvagent/savvagent-cli` → `savvagent/otto`).

## 0.23.1 - 2026-09-06

### Fixed

- Release-note traceability for the `~/.savvagent/config.toml` consolidation shipped in #44. The
  `0.23.0` changelog entry now explicitly documents the breaking removal of the legacy
  `~/.savvagent/language.toml` and `~/.savvagent/theme.toml` files and the move to `[language]`,
  `[theme]`, and `[update]` sections in the shared config file. (#21)

## 0.23.0 - 2026-09-06

### Added

- User-configured `[[mcp_servers]]` in `~/.savvagent/config.toml`, covering local stdio MCP tool
  servers and remote Streamable HTTP MCP servers.
- `/mcp`, a built-in manager screen for listing configured MCP servers, adding/removing entries, and
  showing startup connect status.
- Host-side HTTP tool transport support plus per-endpoint MCP tool-server startup status reporting.
- Keyring support for MCP server secrets under the `mcp:<server name>` account namespace.

### Changed

- **Breaking (`savvagent-host` public API):** `ToolEndpoint::Stdio` gained `name`/`env` fields, a new
  `Http { name, url, auth }` variant was added, and `ToolEndpoint`/the new `HttpAuth` enum are now
  `#[non_exhaustive]`. Any external struct-literal construction of `ToolEndpoint::Stdio` or
  exhaustive `match ToolEndpoint { .. }` must be updated.
- The public host config surface for tool servers now covers both stdio and HTTP transports, and
  the TUI preserves malformed neighboring `[[mcp_servers]]` rows/comments when `/mcp` edits
  `config.toml`.
- **Breaking (on-disk config surface):** language, theme, and self-update preferences now live in
  `~/.savvagent/config.toml` under `[language]`, `[theme]`, and `[update]`. The separate
  `~/.savvagent/language.toml` and `~/.savvagent/theme.toml` files are removed without back-compat
  migration because this repo still has no user base to migrate. `SAVVAGENT_NO_UPDATE_CHECK`
  remains available as an override for CI and scripting. (#21)

## 0.22.1 - 2026-09-06

### Fixed

- **Ratatui footer busy indicator.** Replaced the static working-state status text treatment with a circular `tui-spinner` animation in the terminal footer while a model turn is in progress, while preserving the existing idle footer behavior and footer-slot styling. (#27)

## 0.22.0 - 2026-09-06

### Changed

- **Breaking slash-command rename: `/quit` -> `/exit`.** Savvagent now uses `/exit` as the built-in session-termination command to match common agentic CLI conventions. `/quit` was removed outright (no deprecated alias) under this repo's pre-1.0 SemVer policy, so this ships as a MINOR release. User-defined `commands/exit.md` is now reserved for the built-in command and is skipped with a warning during discovery. (#20)

## 0.21.0 - 2026-09-05

### Removed

- `/view`, `/edit`, and `/editor-keybindings`, plus the built-in TUI/GUI file viewer-editor paths that existed solely to support them.
- Breaking `savvagent-plugin` ABI removal of `ScreenArgs::ViewFile`, `ScreenArgs::EditFile`, and `Effect::SaveActiveFile`. No replacement — plugins should not ask the runtime to open a file-viewer/editor screen; if a plugin genuinely needs this, it should ship its own `Screen` implementation.
- `ratatui-code-editor` and `egui_code_editor`, now that no built-in frontend uses the removed file viewer/editor surface.

## 0.20.2 - 2026-09-05

### Added

- **Added the `tui-engineer` Claude Code skill.** `.claude/skills/tui-engineer/SKILL.md` documents
  project-specific ratatui/crossterm conventions (redraw discipline, the `HostSlot` host-swap
  pattern, `Effect` dispatch boundaries, async event-loop guidance, and TUI test patterns) for
  anyone building or reviewing `crates/savvagent`'s terminal UI. (#24)

## 0.20.1 - 2026-09-05

### Changed

- **Documented the `.claude/skills/` convention.** `CLAUDE.md` now specifies that repo-authored,
  project-specific Claude Code skills (e.g. `rust-engineer`) are committed under
  `.claude/skills/<name>/SKILL.md`, while personal/generic Claude Code skills stay outside the repo
  in the contributor's home directory. Any tracker-related skill must defer to this repo's GitHub
  Issues convention, never JIRA. (#23, shipped in the v0.20.0 tag but missing from that release's
  changelog entry.)

## 0.20.0 - 2026-09-05

### Changed

- **The `/` command palette is now an inline overlay above the prompt.**
  It rendered as a `CenteredModal` — a floating box in the middle of the
  terminal — and is now a `BottomSheet` anchored directly above the prompt
  textarea, matching the inline `/`-list UX of Copilot CLI, Claude Code,
  and OpenCode. The input row stays visible immediately below the list.
  Because the sheet is fixed-height, the command rows are windowed around
  the cursor with an "N more above/below" hint, so the 30+ builtin
  commands stay fully navigable.

### Fixed

- **Plugin screen overlays overflowed their own rect in the egui
  frontend.** Overlays are laid out on a monospace row grid, but the egui
  frontend added its default inter-widget spacing to every rendered line
  and sized the popup's content box to the full outer rect — ignoring the
  frame's inner margin — so the bottom rows of a tall screen were
  silently clipped away by the overlay's own clip rect.

## 0.19.3 - 2026-09-05

### Fixed

- **Shift+drag mouse-capture bypass undiscoverable in-app.** Mouse capture
  (needed for scroll-wheel handling) suppresses native terminal click-drag
  text selection; the only workaround — holding Shift while
  dragging/clicking — was documented solely in the README. The
  `/prompt-keybindings` and `/editor-keybindings` help screens now each
  surface this caveat directly.

## 0.19.2 - 2026-09-04

### Fixed

- **`/update` false "already up to date" reports.** `UpdateState::Unknown`
  (background check hasn't completed yet), `UpdateState::CheckFailed`
  (network/parse error against the GitHub Releases API), and
  `UpdateState::UpToDate` (genuinely current) were all collapsed into the
  identical "Already on the latest version" note, so a check that was
  still in progress or had silently failed was indistinguishable from a
  real "up to date" result. `/update` now returns a distinct, accurate
  message for each state, and the `home.banner` slot now surfaces a
  visible notice when a periodic check fails instead of staying silent.

## 0.19.1 - 2026-09-04

### Fixed

- **Linux `.rpm` packaging.** `package-linux.yml` ran `cargo generate-rpm -p
  savvagent` from the workspace root, but unlike `cargo deb -p`,
  cargo-generate-rpm's `-p <name>` doesn't resolve the crate through
  workspace metadata — it joins the name onto the current directory, which
  fails since this crate lives at `crates/savvagent`. This caused the
  v0.19.0 release to ship without `.deb`/`.rpm` assets. Now runs from
  `crates/savvagent` with an explicit `--target-dir` pointing at the
  workspace's shared `target/`.

### Added

- **Manual re-run support for Linux packaging.** `package-linux.yml` now
  also accepts a `workflow_dispatch` trigger (with a `tag` input), so the
  `.deb`/`.rpm` build+upload can be re-run for an existing release without
  re-running the entire multi-platform `Release` workflow.
- **`RELEASING.md`** documenting the manual release process to use while
  release-plz's automation is blocked by an upstream bug.

## 0.19.0 - 2026-09-04

### Added

- **Web tools (`tool-web`).** New stdio MCP server exposing `web_fetch`
  and `web_search`, bundled as the `savvagent-tool-web` binary.
  `web_fetch` is SSRF-guarded (blocks loopback/private/link-local/
  multicast/CGNAT ranges and IPv4-mapped IPv6 addresses, revalidates
  every redirect hop, restricts to `http`/`https`) and converts HTML
  responses to plain text. `web_search` supports the Brave Search API
  (`SAVVAGENT_BRAVE_API_KEY` / `BRAVE_API_KEY`) or a self-hosted
  SearXNG instance (`SAVVAGENT_SEARXNG_URL`); returns a clear
  "not configured" error otherwise. Both tools default to `Ask`
  permission. Ships with a TUI transcript-summary plugin.
- **Linux packaging.** `cargo-deb` and `cargo-generate-rpm` metadata
  added for the `savvagent` crate, plus a `package-linux.yml` workflow
  that builds `.deb`/`.rpm` artifacts and uploads them to the GitHub
  Release after the existing cargo-dist `Release` workflow completes.
- **Automated releases.** Version bumps, changelog entries, and
  `vX.Y.Z` tags are now generated by
  [release-plz](https://release-plz.dev) from Conventional Commits,
  feeding the existing cargo-dist release pipeline.

### Fixed

- Repository references left over from the `robhicks/savvagent-rs` →
  `savvagent/savvagent-cli` migration — the in-app changelog viewer,
  self-update release check, and self-update installer download all
  pointed at the old repo/branch and would have silently never found
  new releases.

## 0.18.0 - 2026-05-26

### Added

- **External plugins (sub-project D).** WebAssembly Component-Model
  plugins implementing one of three WIT worlds (`plugin-static`,
  `plugin-interactive`, `plugin-provider`) can now be discovered from
  four well-known directories — `<project>/.savvagent/plugins/`,
  `<project>/.claude/plugins/`, `~/.savvagent/plugins/`,
  `~/.claude/plugins/` — hash-trusted via SHA-256 over the whole
  plugin directory tree, and adapted into the live plugin registry
  alongside the built-in plugins. Static plugins contribute slash
  commands, hooks, themes, render slots, and keybindings. Interactive
  plugins own screens (per-open state via Component Model resources).
  Provider plugins ship a new `complete` / `list-models` /
  `count-tokens` provider that joins the `/connect` pool, with
  capability-gated `http` (exact-match `allowed-hosts`),
  `keyring` (account allow-list), and `progress` host imports.
- **Two new crates.** `savvagent-plugin-wit` holds the `.wit` files
  and `wit-bindgen`-generated host bindings (no runtime deps beyond
  `wit-bindgen`); `savvagent-plugin-wasm` holds the wasmtime-backed
  runtime, the three per-world adapters, the manifest parser,
  four-path discovery, the `plugin-trust.toml` ledger, and the
  capability-gated host imports.
- **`/plugins` slash subcommands.** `/plugins install <toml-url>` pulls
  a remote `plugin.toml` (64 KB cap), follows the manifest's `wasm`
  URL (32 MB cap), hashes the staging tree, and opens a trust prompt
  showing manifest fields, source URL, and hash. `/plugins trust`,
  `/plugins revoke`, `/plugins remove`, `/plugins enable`, and
  `/plugins disable` manage the per-id trust state in
  `~/.savvagent/plugin-trust.toml`. `/plugins list` (and the bare
  `/plugins` manager screen) shows every discovered plugin with
  trust status, world, declared exports, and source path.
- **Three-strikes-disable + epoch-interruption trap recovery.** A
  trapping wasm plugin does not unload — the trap is surfaced as
  `PluginError::Unsupported(trap-info)`, the long-lived store is
  dropped, and the next call lazily rebuilds it. After 3 traps within
  a rolling 10-minute window the plugin is auto-disabled with
  `disabled-reason = "repeated-traps"` persisted to
  `plugin-trust.toml`; the user re-enables via
  `/plugins enable <id>`. Per-host-import-call wall-time caps are
  enforced via wasmtime's `epoch_interruption` (default 5 s,
  overridable per-plugin up to 300 s via `[runtime] call-timeout-ms`).
- **Plugin manager screen distinguishes external plugins** with an
  `(external)` label so it is obvious which rows came from `.wasm`
  vs the built-in registry.
- **Three runnable example plugins** under `examples/`:
  `plugin-hello-static/` (the `/hello` slash command),
  `plugin-hello-interactive/` (renders `Hello, world!` from a wasm
  screen), `plugin-hello-provider/` (echo provider that returns the
  last user message). Each example is deliberately outside the
  workspace so `cargo component build`'s profile setup doesn't
  clash with the workspace release config; a supplemental CI job
  exercises them via `cargo component build`.

### Dependencies

- `wasmtime = "34"` (and `wasmtime-wasi = "34"`) pinned at the
  workspace level. Default features cover Component-Model + async,
  which is exactly what the runtime needs.

### Notes

- Sub-projects A (user slash commands), B (user-defined hooks), and
  C (user-defined agents) shipped together as v0.17.0 immediately
  before this release. v0.18.0 is sub-project D — the WIT-portable
  plugin runtime v0.9.0 was always designed for.
- See [`docs/superpowers/specs/2026-05-25-external-plugins-design.md`](docs/superpowers/specs/2026-05-25-external-plugins-design.md)
  for the canonical design rationale (trust model, capability
  matrix, non-goals) and
  [`docs/plugins/authoring.md`](docs/plugins/authoring.md) for the
  long-form author's guide (quickstart, WIT reference, capability
  table, three-strikes recovery, v0.18.0 limitations).

## 0.17.0 - 2026-05-23

### Added
- User-defined slash commands. Drop markdown files under
  `.savvagent/commands/` (project), `.claude/commands/` (project-claude),
  `~/.savvagent/commands/`, or `~/.claude/commands/`; each becomes a
  slash command. Frontmatter supports `description`, `argument-hint`,
  `model`, and (forthcoming) `allowed-tools`. Body templating supports
  `$ARGUMENTS`, `$1`/`$N`, `@<file>`, and `!<cmd>`. Project-local
  commands that use `!<cmd>` are gated behind a first-run trust prompt
  whose decisions persist to `~/.savvagent/trusted-projects.json`.
  `/reload-commands` rescans directories after edits.
- User-defined hooks. Drop a Claude-Code-compatible `settings.json`
  under `.savvagent/` (project), `.claude/` (project-claude),
  `~/.savvagent/`, or `~/.claude/`; the `hooks` block contributes shell
  hooks for `PreToolUse`, `PostToolUse`, `UserPromptSubmit`,
  `SessionStart`, and `Stop`. `PreToolUse` and `UserPromptSubmit`/`Stop`
  can block (`exit 2` with stderr reason, or stdout JSON
  `{"continue":false,"stopReason":"…"}`). `UserPromptSubmit` hooks can
  inject `additionalContext` that gets prepended to the user's prompt
  before the model sees it. `/reload-hooks` rescans without restart.
- **User-defined agents.** Drop markdown files under `.savvagent/agents/`,
  `.claude/agents/`, `~/.savvagent/agents/`, or `~/.claude/agents/`; each
  becomes a subagent the parent model can spawn via a new built-in
  `task` tool. Frontmatter supports `description` (required), `tools`
  (comma-separated string or YAML list — exact-name allowlist; absent
  inherits parent's tool set, `[]` allows only `task`), `model` (per-agent
  override), and `name`. Body `@<path>` includes are expanded once at
  load time. The `task` tool is registered only when ≥1 agent is
  discovered; its `subagent_type` enum is populated from the index and
  refreshed by `/reload-agents`. Subagent execution gets its own Sub-Host
  with own session state, system prompt, model selection, and filtered
  tool view; shares the parent's `ProviderClient`, `ToolRegistry`,
  `PreToolUseGate`, permissions, and sandbox via `Arc`. Tool scoping is
  enforced both at the provider boundary (filtered `ToolDef` list) and
  at runtime (`ScopedToolRegistry` rejects out-of-allowlist names).
  `SAVVAGENT_AGENT_MAX_DEPTH` (default 3) caps subagent recursion.
- `HookKind::SubagentStop` event. Fires after each subagent reaches a
  clean `end_turn` (not on cancellation). User shell hooks subscribe via
  the same `settings.json` shape; payload includes `subagent: "<name>"`
  and `stop_hook_active`. `stop_hook_active` is hardcoded `false` in v1
  (re-prompt mechanism is a future follow-up matching the parent `Stop`
  pattern).
- `PreToolUse` / `PostToolUse` stdin payloads gain an optional
  `subagent: "<name>"` field. Absent for parent-turn calls (backward
  compatible); present with the agent name for subagent-originated
  tool calls. Threaded via a `tokio::task_local!` set by `SubHost`
  during dispatch.
- Transcript schema v2. Adds a top-level `subagent_transcripts: HashMap<String, SubagentTranscript>`
  sidecar keyed by parent `task` tool-call id. v1 transcripts load
  cleanly (with a one-time warn-log; the sidecar is empty).
- New built-in plugins: `internal:user-agents` (discovery + `task` tool
  registration + `/reload-agents`) and `internal:tool-task-summary`
  (one-line summaries for the `task` tool in the conversation log).
- `Effect::RegisterInProcessTool` variant. Savvagent-internal extension
  point that lets built-in plugins contribute in-process tool handlers
  (host-local; not exposed to WASM plugins).
- `Host::session_id()` accessor and a `SAVVAGENT_AGENT_MAX_DEPTH` env
  override are now public surfaces.

### Changed

- `ToolRegistry::call_with_bash_net_override` now rejects in-process
  tool names with a guardrail error — those must be dispatched via
  `call_in_process` (the parent turn loop and SubHost both do).
- `Host::tools` is now `Mutex<Option<Arc<ToolRegistry>>>` (was
  `Mutex<Option<ToolRegistry>>`) so `Arc` clones can be shared with
  `SubHost`. Shutdown is unchanged in observable behavior; internally
  uses `Arc::into_inner` and falls back to drop-time cleanup if a
  subagent's Arc is still live.

### Changed
- User-hook stdin payloads now carry a real `transcript_path`
  (`<transcript_dir>/<session_id>.json`) instead of an empty string. The
  path is established at startup and reused for the lifetime of the
  session; `save_transcript_now` writes to the same file so auto-saves
  and manual `/save` coalesce into one transcript per session (previously
  each save created a new timestamped file).
- User-hook `Stop` payloads now carry the real `stop_hook_active` flag:
  it stays `false` on the first `Stop` dispatch in a turn and flips to
  `true` on any subsequent `Stop` dispatch following a `Block` decision
  until the next `TurnStart` clears the latch. Matches Claude Code's
  contract so Stop hooks can detect "the agent already tried to stop
  once" once savvagent gains a re-run-on-Stop-block mechanism.

## 0.17.0 - unreleased

> Part of the inline HTML canvas initiative. Per the repo's multi-phase
> release convention (`feedback_phase_release_rollup`), **no git tag is
> pushed for 0.17.0** — the final tag (v0.18.0) goes up after Phase 2
> (mouse + keyboard interaction) lands. See
> `docs/superpowers/specs/2026-05-21-inline-html-canvas-design.md` and
> `docs/superpowers/plans/2026-05-21-inline-html-canvas-phase-1.md`.

### Added

- **SPP v0.2.0**: new `ContentBlock::Html { source }` content block and
  `BlockDelta::HtmlSourceDelta { source }` stream delta. Additive: v0.1.0
  conformance is preserved.
- **`savvagent-fence` crate**: streaming parser that extracts
  ```` ```html-canvas ```` fenced blocks from model text output. Wired
  into all four providers (`provider-anthropic`, `provider-gemini`,
  `provider-openai`, `provider-local`).
- **`savvagent-canvas` crate**: Blitz-backed `HtmlCanvas` implementing
  `ContentRenderer` for static rendering (Phase 1). Pinned to
  `blitz-{dom,html,paint,traits} = 0.3.0-alpha.4` +
  `anyrender 0.10` / `anyrender_vello_cpu 0.12` / `peniko 0.6`. Includes
  a subset validator that emits `tracing::warn!` for elements outside
  the canvas subset (e.g., `<script>`, `<iframe>`, `<details>` with its
  Phase 1 paint-regardless-of-open quirk).
- **Plugin trait surface extensions** (`savvagent-plugin`):
  `ContentRenderer` trait + supporting types (`Frame`, `PixelSize`,
  `PixelFormat`, `ContentBlockId`, `InputEvent`, `MouseEventPortable`,
  `FocusableElement`, `InputOutcome`, `Rect`, `FocusKind`,
  `MouseEventKind`, `MouseButton`); `Plugin::create_renderer` factory;
  `Contributions::content_renderers` + `Contributions::prompt_segments`;
  `SlashSpec::suppress_prompt_segments`;
  `Effect::OpenUrl { url, target }` + `UrlTarget`; `SystemPromptSegment`;
  `PluginError::ContentRendererNotFound`.
- **Host prompt-segment composition** (`savvagent-host`):
  `Host::set_prompt_segments` and `Host::set_turn_suppression` allow
  the TUI to compose plugin-contributed `SystemPromptSegment`s into
  the model's system prompt, with per-slash suppression for slashes
  whose output is destined for non-canvas surfaces.
- **`internal:html-canvas` built-in plugin**: claims SPP `"html"` blocks
  as the canonical renderer; contributes the default system prompt
  segment instructing models to use ```` ```html-canvas ```` fences for
  structured documents; ships the `/save-canvas [path] [--block N]
  [--open]` slash command.
- **Auto-export**: every finalized HTML canvas is written to
  `~/.savvagent/canvases/<unix>-<turn>-<block>.html` (mode 0o600,
  directory 0o700). Disable by toggling the `internal:html-canvas`
  plugin off in `plugins.toml`; there is no separate auto-export
  toggle in v0.17.0.
- **TUI inline canvas rendering**: `Entry::Canvas` variant +
  `CanvasRegistry` on `App` holds live renderer instances; rendering
  uses `ratatui-image 11.0.2` (kitty / iTerm2 / WezTerm / Ghostty /
  sixel). Streaming HTML blocks show a typewriter-style source preview
  during stream and swap to the rendered canvas on `ContentBlockStop`.
  Terminals without an image protocol show source code with a banner.
- **Docs**: `docs/canvas-terminal-compat.md` (supported terminals +
  tmux passthrough setup). README updated with the inline HTML
  rendering blurb, `/save-canvas` row, and `~/.savvagent/canvases/`
  persistence entry.

### Changed

- **MSRV bump**: workspace `rust-version` raised from `1.85` to `1.89`
  to accommodate Blitz's transitive deps (notably `stylo`).
- **Workspace deps**: added `savvagent-fence`, `savvagent-canvas`,
  `blitz-dom`, `blitz-html`, `blitz-paint`, `blitz-traits`, `anyrender`,
  `anyrender_vello_cpu`, `peniko`, `ratatui-image`, `image`.
- **`ratatui-image` features**: `chafa-dyn` is disabled (libchafa not
  required); `crossterm` + `image-defaults` retain sixel / kitty /
  iTerm2 / halfblocks support. Terminals with sixel patches (including
  Alacritty-with-sixel) work via the crossterm backend.
- **Anthropic stream translator**: introduced a separate
  SPP-output block-index space (`upstream_to_local` map) so a single
  upstream Text block can fan out into multiple SPP Text/Html blocks
  across fence transitions. `ContentBlockStart` for Text is now
  emitted lazily on the first delta to keep the wire shape consistent.

### Fixed

- `savvagent-fence::finish` correctly handles the closing fence
  arriving without a trailing newline (was incorrectly setting
  `unclosed_fence: true`).
- `savvagent-fence::push` eagerly flushes partial-line tails that
  provably can't form a fence-marker prefix; per-token streaming no
  longer stalls until a newline.
- `provider-local::translate` no longer drops `ContentBlock::Html`
  blocks from echoed conversation history (multi-turn canvas
  conversations against Ollama preserve prior canvas content).
- `HtmlCanvas` uses `blitz_dom::StyleThreading::Sequential` to prevent
  Blitz's default Parallel threading from panicking against Stylo's
  global thread pool under concurrent renders (Blitz #430).
- `HtmlCanvas::render` clamps width and natural-height to
  `u16::MAX` and logs `tracing::warn!` to avoid silent truncation
  inside `vello_cpu`'s u16 dimensions.

### Notes

- Phase 1 is static rendering only — no interactivity inside the
  canvas. Phase 2 will add mouse + keyboard interaction, focus
  management, soft freeze, and the `Ctrl-O` open-in-browser
  keybinding. Phase 2 will ship as v0.18.0 and is the release that
  carries the v0.18.0 git tag (per the multi-phase rollup
  convention).
- Pre-existing `clippy::collapsible_if` violations across the workspace
  are temporarily allowed at the crate level under rustc 1.95's
  enhanced lint stringency. To be cleaned up in a follow-up pass.

## 0.16.1 - 2026-05-21

### Added

- **Live install-progress modal for `/lsp`** (`internal:lsp-installer` plugin). After confirming the picker, savvagent now opens a modal that streams per-server status (`queued → downloading… X.X MB / Y.Y MB → verifying SHA256… → extracting / running npm → installed / failed`) instead of waiting silently for the whole batch. The progress screen owns the install driver via `tokio::spawn` and reads shared `Arc<Mutex<ProgressState>>` on every render; the TUI's existing 50 ms render cadence keeps the UI fresh with no new `Effect`/`HostEvent`/`WorkerMsg` plumbing.

### Changed

- The `/lsp` picker's `Enter` no longer routes through the synchronous `/lsp __install` slash sub-command. It now opens the new `lsp_installer.progress` screen with the selected catalog ids. The legacy `__install` sub-command remains callable for external dispatchers (CI smoke, future keybindings).

### Fixed

- Terminal-state guard in `apply_notification` prevents a late-arriving `InstallProgress::Downloading` from un-failing an entry that the `Err` arm of `run_installs` just marked `Failed`.
- `run_installs::Ok(outcome)` writes `Installed` directly into the shared state (instead of relying on the spawned `Done` notification), eliminating a race against `spawn_driver`'s outcomes collection.
- Picker batch-abort displays the actual stored reason (`"batch aborted after SHA mismatch"`) for downstream entries, rather than misleadingly reporting them as individual SHA-mismatch failures.
- "Restart savvagent to pick up the new servers" suggestion is suppressed when zero servers successfully installed.
- `create_screen` rejects unexpected `ScreenArgs` variants instead of silently falling back to an empty entry list.
- `tracing::warn!` on id-miss in `run_installs` state lookups (previously silent no-ops).

### Notes

- Sequential installs (v1). Esc dismisses the modal; the install continues in the background. On finish, Enter dismisses and emits summary `PushNote`s so the install record persists in the conversation log after the modal closes.

## 0.16.0 - 2026-05-21

### Added

- **`/lsp` slash command** (`internal:lsp-installer` plugin). Opens a
  multi-select picker over a curated catalog of language servers
  (`rust-analyzer`, `lua-language-server`, `typescript-language-server`,
  `pyright`, `bash-language-server`, `vscode-langservers-extracted`). On
  confirm, savvagent downloads pinned binaries (SHA256-verified) or runs
  `npm i -g` for Node-based servers, then merges entries into
  `~/.savvagent/lsp.toml`. Binaries land in `~/.savvagent/lsp-bin/<id>/`.
- **Reusable `MultiSelectList<T>` widget** under
  `crates/savvagent/src/plugin/widgets/`. Generic cursor + filter +
  selection-by-stable-id state machine; `Confirm` returns selected items
  in catalog order regardless of selection sequence. The `/lsp` picker
  is its first consumer; future multi-select pickers in other plugins
  can wrap it.
- **MCP resource subscriptions in `savvagent-host`**. Every connected tool server is now constructed with a `ResourceCapturingHandler` (rmcp `ClientHandler`) that forwards `notifications/resources/updated` and `notifications/resources/list_changed` into a host-owned mpsc channel. A new `resource_pump` task drains the channel into `ResourceCache` and emits the new `TurnEvent::ResourceUpdated { uri, owner, summary }` so the TUI can render a banner.
- **Built-in `read_resource` synthetic tool**. Always advertised in `ToolRegistry::defs`, takes `{ uri: string }`, and routes through the cache to call `resources/read` on the URI's owning tool server.
- **Iteration-boundary conversation injection**. At the start of every tool-use-loop iteration, dirty URIs are drained from `ResourceCache` and appended as `Message{role:User, content:Text}` blocks of the form `[resource updated: <uri>]`. The model decides whether to call `read_resource` on any of them.
- **`tool-lsp` crate**. New stdio MCP server that wraps user-configured LSP servers behind seven MCP tools (`lsp_definition`, `lsp_references`, `lsp_hover`, `lsp_document_symbols`, `lsp_workspace_symbols`, `lsp_rename`, `lsp_code_actions`) and publishes diagnostics as MCP resources (`lsp://diagnostics/<absolute-path>`). Configured via `~/.savvagent/lsp.toml` + `<repo>/.savvagent/lsp.toml`; no languages are hardcoded.
- **`savvagent-tool-lsp` binary**. Shim that delegates to `tool_lsp::run`; bundled alongside `savvagent-tool-fs`/`-bash`/`-grep` in the release archive.
- **`lsp-types` workspace dep** at version 0.97.

### Notes

- Diagnostics flow as `[resource updated: lsp://diagnostics/<path>]` notes the model can pull via `read_resource`.
- `lsp_rename` and `lsp_code_actions` return *edit descriptors only*; the model applies via `tool-fs::write_file`.
- `WorkspaceEdit`s that include file rename/create/delete or version-tagged edits are rejected (v1 supports plain in-file text edits only).
- Idle LSP sessions evicted after ten minutes (`tool_lsp::IDLE_TIMEOUT`).
- No default `lsp.toml` ships — see README for copy-pasteable rust/typescript/python/go examples.

## v0.15.0 — Multi-provider pool + auto-routing + TUI polish (2026-05-20)

This release rolls up Phases 1–6 of the multi-provider initiative
(connection pool, cross-vendor `tool_use_id` compatibility gate,
`@provider:model` override, modality-aware routing, user-edited routing
rules, heuristic classifier) along with subsequent TUI polish that
landed before tagging (mouse-wheel scrolling, tool-call summaries,
conversation-log auto-tail). The in-tree per-phase
`release(0.16.0)`–`release(0.20.0)` commits and the unreleased 0.21.0
CHANGELOG scaffold were consolidated here; no intermediate version was
tagged between v0.14.3 and v0.15.0.

### Phase 1 — Multi-provider connection pool

#### Added

- **Multi-provider connection pool.** `/connect <provider>` is now silent when
  the keyring already has a stored key; the API-key modal only opens when a key
  is missing or `--rekey` is passed. Multiple providers can be connected
  simultaneously; the pool is an `Arc`-held `HashMap<ProviderId, PoolEntry>` in
  the host.
- **`/disconnect <provider> [--force]`** removes a provider from the pool. Drain
  mode (default) waits for any in-flight turn to finish; Force mode signals
  cooperative cancel, waits 500 ms, then aborts. `TurnEvent::Cancelled { reason }`
  and `TurnEvent::AbortedAfterGrace { reason }` (with
  `CancellationReason::ProviderDisconnected` and `UserAbort`) emit during
  force-disconnect.
- **`/use <provider>`** switches the active provider. (Phase 1 cleared the
  conversation on switch; Phase 3 made cross-provider history safe — see below.)
- **`~/.savvagent/config.toml`** for startup connection policy and per-provider
  connect timeout. Schema:

  ```toml
  [startup]
  # "opt-in" (default), "all", "last-used", "none"
  policy = "opt-in"
  startup_providers = ["anthropic"]
  connect_timeout_ms = 3000

  [migration]
  v1_done = true
  ```

- **First-launch migration picker.** Users upgrading with multiple stored keys
  see a one-time picker that selects which providers to connect on startup; the
  selection is persisted to `~/.savvagent/config.toml` under `startup_providers`.
  Single-key users see no UI change.
- **`Alt-Enter` on the `/connect` picker** re-enters the API key for the focused
  provider (equivalent to typing `/connect <id> --rekey`).
- **Status bar active-provider marker.** The active provider in the footer is
  prefixed with `▸ `; pool members that are connected but inactive are listed
  without the marker.

#### Changed

- **`/connect <provider>`** no longer replaces the active host or clears the
  conversation. The new provider joins the pool additively; use `/use <provider>`
  to switch the active turn context.
- **`SAVVAGENT_MODEL`** accepts both legacy bare-model form
  (`claude-opus-4-7`) and new `provider/model` form
  (`anthropic/claude-opus-4-7`); ambiguous bare forms log a warning and
  fall back to the active provider's default.

#### Migration notes

- Pre-existing users with multiple stored keys see a one-time picker on first
  launch; the selection writes `startup_providers` to
  `~/.savvagent/config.toml`. Single-key users see no UI change beyond the
  silent re-connect behavior.
- Users who relied on `/connect <other>` to switch providers should now use
  `/use <other>`.

#### Internal

- `ProviderId` moved from `savvagent-plugin` to `savvagent-protocol`;
  re-exported from `savvagent-plugin` for backwards compatibility.
- New types in `savvagent-host` and `savvagent-protocol`:
  `ProviderCapabilities`, `ModelCapabilities`, `ModelAlias`, `CostTier`,
  `ProviderRegistration`, `StartupConnectPolicy`, `PoolEntry`, `ProviderLease`,
  `DisconnectMode`, `PoolError`, `LegacyModelResolution`.
- `Host` gains: `add_provider`, `remove_provider`, `set_active_provider`,
  `active_capabilities`, `is_connected`.
- `TurnEvent::Cancelled { reason }` and `TurnEvent::AbortedAfterGrace { reason }`
  with `CancellationReason::ProviderDisconnected` and `UserAbort`.
- `HostEvent::ActiveProviderChanged` (and matching `HookKind`) fires on `/use`
  and startup so plugin slot rendering stays in sync.

### Phase 2 — Cross-vendor `tool_use_id` compatibility gate

#### CI

- **Cross-vendor `tool_use_id` compatibility gate.** New
  `crates/savvagent-host/tests/cross_vendor_history.rs` integration test
  validates that every `(sender_provider, receiver_provider)` pair across
  the three shipping vendors (Anthropic, Gemini, OpenAI) accepts SPP
  history whose `tool_use_id` is prefixed with the originating provider
  (e.g. `"anthropic:toolu_xyz"`). Nine pair tests run in PR CI against
  axum-backed mock vendor servers via the dedicated `cross-vendor-gate`
  job with `--no-fail-fast`, so any regression surfaces every failing
  pair. `#[ignore]`-marked live-vendor twins are runnable manually via
  `cargo test -p savvagent-host --test cross_vendor_history -- --ignored`
  with `ANTHROPIC_API_KEY` / `GEMINI_API_KEY` / `OPENAI_API_KEY` set.

#### Internal

- Phase 2 of the multi-provider-pool roadmap (see
  `docs/superpowers/specs/2026-05-15-multi-provider-pool-and-auto-routing-design.md`).
  No user-visible runtime behavior changes; this release establishes the
  release-gate Phase 3 (cross-provider routing with `@provider:model`
  overrides) depends on. A live-vendor nightly workflow is intentionally
  deferred to a follow-up.

### Phase 3 — `@provider:model` override + cross-provider conversations

#### Added

- **`@provider:model` (and `@provider`, `@alias`) prefix.** Users can now
  route an individual turn to a specific provider/model by prefixing
  their message with `@anthropic:claude-opus-4-7`, `@gemini`, `@opus`,
  etc. Unknown `@`-tokens are NOT consumed — the message goes through
  verbatim and the next turn routes to the active provider as usual. To
  start a message with a literal `@`, prefix with `@@`.
- **Per-turn routing badge.** Each assistant turn now renders a muted
  `▸ provider/model — Reason` line above its response so it's always
  obvious which provider handled the turn and why
  (Override / Default; modality / rules / heuristics arrive in later
  phases).
- **Built-in model aliases.** `@opus`, `@sonnet`, `@haiku` map to
  Anthropic; `@flash`, `@pro` map to Gemini; `@gpt`, `@gpt-4o` map to
  OpenAI. Ambiguous aliases (same short name across providers) fall
  through with a styled note rather than picking one silently.

#### Changed

- **Cross-provider history is now safe.** `/use <provider>` no longer
  clears the conversation when switching the active provider. The host
  namespaces every `tool_use_id` with the issuing provider at insertion
  time (`<provider_id>:<original_id>`) and strips the receiver's own
  prefix back off before each request; foreign-prefixed ids flow through
  every translator as opaque strings, validated by the Phase 2
  cross-vendor gate.
- **`/model` picker shows every connected provider's models.** Selecting
  a model from a different provider updates both the active provider and
  the default model in one step.

#### Internal

- Phase 3 of the multi-provider-pool roadmap. New
  `crates/savvagent-host/src/router/{prefix,router,namespace}.rs`
  modules; `Host::run_turn_inner` now parses the `@`-prefix, invokes
  `Router::pick`, emits `TurnEvent::RouteSelected`, and namespaces ids
  on append / strips on egress.
- `TurnEvent::RouteSelected { provider_id, model_id, reason }` added.
  Existing `TurnEvent` consumers that match the enum exhaustively need a
  new arm (the TUI's `apply_turn_event` handles it; downstream consumers
  outside this repo may need to update).
- `PoolEntry` gains an `aliases` field carrying every `ModelAlias` the
  provider's `ProviderRegistration` declared; `PoolEntry::new` takes a
  new `aliases: Vec<ModelAlias>` argument.

### Phase 4 — Modality-aware routing

#### Added

- **Automatic modality routing for image inputs.** When your message
  contains an image and the active provider's chosen model doesn't
  support vision, the router auto-redirects the turn to a sibling
  model on the **same provider** that does. Cross-provider redirects
  are not done automatically — that crosses a billing boundary the
  user picked. The transcript badge shows `Modality(image)` when a
  same-provider redirect happens.
- **`TurnEvent::ModalityWarning`.** Surfaced as a muted note in the TUI
  when an image is attached but the active provider has no
  vision-capable model, or when an `@`-override pinned a
  vision-incapable model. The request still runs; the warning
  explains why the next call may fail.
- **`Host::run_turn_streaming_with_blocks(content, events)`.** Public
  entrypoint that accepts a user turn as a `Vec<ContentBlock>` instead
  of a string, so a future image-upload UX can deliver image blocks
  without going through the text-only path.

#### Internal

- Phase 4 of the multi-provider-pool roadmap. New
  `crates/savvagent-host/src/router/modality.rs` module with field
  names (`has_image`, `has_pdf`, `has_audio`) aligned to Phase 5's
  `routing.toml` predicates so the user-rules layer can bind to the
  same struct without rename. `Router::pick` takes a new
  `RequiredModalities` argument; the `#[non_exhaustive]`
  `RoutingReason` enum gains a `Modality { kind }` variant.

### Phase 5 — User-edited routing rules

#### Added

- **User-edited routing rules** (`~/.savvagent/routing.toml`). Routes a turn to a specific `provider/model` based on per-turn predicates (`has_image`, `keywords`, `max_input_chars`, `min_input_chars`). Layer 3 of the multi-provider router, between modality redirects and the default model.
- **`/route show`** prints the active rules, the default, the heuristics-enabled state, and the most recent routing decision.
- **`/route reload`** re-reads `routing.toml` without restarting the TUI. Parse errors keep the prior rules in place and surface a styled note.
- **`routing.toml#default`** is consulted between `~/.savvagent/models.toml` and the provider's hard-coded default during model resolution. Env (`SAVVAGENT_MODEL`) and `models.toml` still take precedence; routing.toml's default replaces the provider's built-in fallback when neither higher layer applies.

#### Changed

- `Router::pick` now takes `rules: &RoutingRules` and `user_text: &str` parameters (additive).
- `RoutingReason` gains a `Rule { name }` variant rendered as `Rule(<name>)` in the transcript badge.

### Phase 6 — Heuristic classifier (Layer 4)

#### Added

- **Heuristic classifier (Layer 4 of the router)**. Opt-in via `heuristics = true` in `~/.savvagent/routing.toml`. Short questions (≤200 chars + `?`) route to a cheap model (`CostTier::Free` or `Cheap`); coding-flavored prompts (substring match against `refactor`, `implement`, `debug`, `fix bug`, `compile`, `stack trace`, `function`, `class`, `error`) route to a premium model (`Premium` or `Standard`). Same-provider preferred; sibling providers are walked only when the active provider has no matching model. Off by default.
- **`RoutingReason::Heuristic { kind }`** variant on the existing `#[non_exhaustive]` enum. Transcript badge renders `Heuristic(short)` / `Heuristic(coding)`.
- **`/route show`** now describes the active classifier (categories + triggers) when `heuristics = true`. When `heuristics = false`, no heuristic line is printed.

#### Changed

- `Router::pick` runs a new Layer-4 step between rules and default. Layered precedence is unchanged: `@`-override, Modality, and matching user Rules all still beat the heuristic when they apply.

### TUI polish (post-Phase 6)

#### Added

- **Mouse-wheel scrolling on the conversation log.** The TUI now enables crossterm's `EnableMouseCapture`, and wheel ticks adjust `App::log_scroll_offset_from_bottom` exactly like `PageUp`/`PageDown` — three wrapped rows per notch. Scrolling down past the live tail snaps back to auto-tail (`None`), the same contract `PageDown` honors, so a mixed wheel + keyboard session never lands in the ambiguous "parked at bottom of scrollback" state. Gated to the home screen (no plugin screen on top, no modal, no file picker, no splash) so popups and pickers keep their existing key-driven behavior.
- **Tool-call summaries + JSON highlighter in the conversation log** (#89). Tool invocations render as a compact one-line summary with a syntax-highlighted JSON argument preview, improving scan-ability when many tool calls fire in a single turn.
- **Conversation-log auto-tail + scrollback with `PageUp`/`PageDown`** (#87). The log auto-tails to the newest message by default and only stops auto-tailing when the user explicitly scrolls back; `PageDown` past the live tail re-arms auto-tail.
- **Per-project prompt history.** The prompt input now recalls prior messages with `Up`/`Down`, scoped per project root so unrelated workspaces don't bleed into each other's history.
- **Dynamic model catalog from `list_models` on `/connect`** (#86). The provider registration step now consults the provider's runtime `list_models` MCP method (where available) so the `/model` picker reflects what the provider actually serves, instead of the hard-coded catalog baked into the binary.

#### Fixed

- `/connect` now makes the just-connected provider active (no more "connected but not selected" footgun).
- `/model` picker is populated on startup when a bootstrap host already exists.
- The silent-connect path (no API-key modal) now adds the provider to the host pool.
- `routing.toml#default` is validated on load, and the `/model` picker refreshes on every `/connect`.

### Notes

- Enabling mouse capture means terminals no longer get raw wheel events for native text selection. To select text inside the TUI, hold **Shift** while dragging — every major terminal emulator falls back to native selection while Shift is held.
- Coding-keyword matching in the heuristic classifier is **substring-based** in v1 — `function` matches `functional`, `error` matches `terror`. Users who want stricter matching write explicit `[[rule]]` entries; rules run earlier (Layer 3) and beat the heuristic.
- The `routing.show-heuristics-pending` locale key remains in the catalog for backward compat but is no longer emitted by any code path.

## v0.14.3 — Self-update plugin re-checks GitHub Releases every 2 hours (2026-05-15)

### Fixed

- **Long-running TUI sessions now notice new releases.** Previously,
  the `internal:self-update` plugin only consulted the GitHub Releases
  API when the `HostStarting` hook fired (i.e., at TUI launch), so a
  session that stayed open for days would never observe a release
  published mid-session. The spawned check task now runs on a
  `tokio::time::interval` with a 2-hour cadence: the first tick
  preserves today's startup behavior (and the 24h on-disk cache at
  `~/.savvagent/update-check.json`), while subsequent ticks bypass the
  cache and re-query GitHub. New releases auto-install in exactly the
  same way as the startup path; the banner shows the progression and
  the existing restart hint fires on exit.

  The 2-hour interval is fixed (no env var override yet — file an
  issue if you need one). `MissedTickBehavior::Delay` is set so a
  suspended laptop or a long install never produces a burst of
  catch-up network calls when the system resumes.

  Skip rules per tick:
  - `Disabled` / `Updated` end the loop (opt-out, or binary already
    swapped and awaiting restart).
  - `Installing` skips the tick — covers the case where the user
    typed `/update` and the dispatcher task is parked on the
    installer.
  - `InstallFailed { latest: T }` re-runs the check on every tick to
    pick up any *newer* release GitHub publishes, but skips the
    install when the live check still resolves to `T`. This avoids
    hammering a known-broken release while still recovering
    automatically once a new release lands.

  Closes #78.

## v0.14.2 — Gemini tool calls no longer abort with "stop_reason=end_turn but tool_use block(s) present" (2026-05-15)

### Fixed

- **Gemini tool calls now actually dispatch.** Any Gemini-routed turn
  that asked the model to invoke a tool (e.g. *"Do we have any issues
  on GitHub?"* on top of a shell tool) was aborting the turn with
  `Error: malformed assistant response: stop_reason=end_turn but 1
  tool_use block(s) present`. Root cause was a two-layer mismatch:
  Gemini's `generateContent` API has no distinct `TOOL_USE` finish
  reason and emits `finishReason="STOP"` even when the candidate carries
  a `functionCall` part, but the host's `Host::run_turn_streaming` loop
  treated `stop_reason=EndTurn` alongside `tool_use` content blocks as a
  hard `HostError::MalformedResponse` and dropped the turn. Three fixes
  ship together:
  - The Gemini translator (`translate.rs::response_from_gemini`) and
    streaming accumulator (`stream.rs`) now force `StopReason::ToolUse`
    whenever the assistant content carries any `ToolUse` block,
    regardless of the upstream `finishReason`. The override is applied
    in every accumulator entry point (`consume_chunk`, `flush`, `finish`)
    via a new `Accumulator::coerce_tool_use_stop_reason` helper so it
    survives Gemini's occasional split-chunk streaming order (function
    call in one chunk, `finishReason="STOP"` in a later chunk). When
    overriding away from a `Refusal`/`Other` finish reason the provider
    now `tracing::warn!`s with the original reason so safety / malformed
    signals aren't silently lost.
  - `Host::run_turn_streaming` now treats the presence of `tool_use`
    blocks as authoritative: when any are present the host runs them
    and continues the tool-use loop regardless of the provider-reported
    `stop_reason`. Defensive against other providers with the same
    `finishReason` conflation.
  - When a turn terminates with a non-`EndTurn` stop reason and no
    tool calls (`MaxTokens`, `Refusal`, `StopSequence`, `Other`), the
    host now `tracing::warn!`s instead of swallowing the anomaly at
    `debug!`. The clearest hazard is a `MaxTokens` cutoff committing a
    truncated assistant turn to session state — at least it's loud
    now. Surfacing this in `TurnOutcome` for the TUI is a separate
    follow-up.

  Closes [#76](https://github.com/robhicks/savvagent-rs/issues/76).

### Deprecated

- `savvagent_host::HostError::MalformedResponse` is no longer
  constructed. The variant is preserved for the public API surface and
  marked `#[deprecated]` (slated for removal in 0.15.0).

## v0.14.1 — self-update cache invalidation on out-of-band upgrade (2026-05-14)

### Fixed

- **`/update` no longer reports "Already on the latest version" when a
  newer release is genuinely available.** The 24-hour update-check cache
  at `~/.savvagent/update-check.json` was being trusted on every launch
  inside its TTL, even when the running binary had moved *past* the
  cached `latest_tag`. A user who launched while on `v0.12.0` (cache
  recorded `latest_tag: v0.12.0`), then upgraded out-of-band to `v0.13.0`
  via `cargo install` / a downloaded tarball / a package manager, would
  start `v0.13.0` and have the plugin classify `0.13.0 > v0.12.0` as
  `Ahead → UpToDate` — silently hiding any newer release (`v0.14.0` in
  this case) that GitHub had since published. The plugin now treats a
  cache whose tag is older than (or unparseable against) the running
  binary as a cache miss and re-fetches, restoring the auto-install +
  banner + `/update` path. Unparseable cached tags are also re-fetched
  rather than swallowed as `CheckFailed`.

## v0.14.0 — default system prompt + tool-description trust boundary (2026-05-14)

### Added

- **Default system prompt.** The host now builds and attaches a dynamic
  system prompt at every session start, introducing Savvagent's
  identity, behavior expectations, the names of wired tools, the
  environment (OS, project root, git presence, version), and output
  conventions. Closes the long-standing failure mode where the model
  would claim "I cannot access GitHub" despite having a shell tool with
  network access available — the prompt now explicitly translates
  "shell wired" into "gh / curl / git / rg / package managers are
  available, use them."
- `HostConfig::with_app_version` lets embedders pass the binary's
  `CARGO_PKG_VERSION` so the prompt's Environment section shows the
  installed version (not the `savvagent-host` crate version). The TUI
  wires this automatically; library embedders can pass any
  `impl Into<String>`.
- `HostConfig::with_default_prompt_disabled` lets embedders that want
  to fully own the system message suppress the built-in default layer.

### Changed

- `project::system_prompt` removed; replaced by the 3-layer
  `project::layered_prompt` (default → embedder override →
  `SAVVAGENT.md` body). All three layers compose with H1 section
  headers; whitespace-only layers are silently dropped; non-empty
  layers render verbatim so code fences and indentation in
  `SAVVAGENT.md` survive.

### Breaking for embedders

- Default-installed embedders now receive a non-empty system prompt
  even when they did not set `HostConfig::system_prompt`. To preserve
  the previous "empty system field" behavior, call
  `HostConfig::with_default_prompt_disabled()` AND leave
  `system_prompt` unset AND point `project_root` at a directory with
  no `SAVVAGENT.md`.

### Security

- Tool descriptions supplied by MCP tool servers are no longer
  promoted into the system prompt. Names are listed under an
  explicitly-framed informational block as code spans; descriptions
  still flow to the model via the request's typed `tools` field.
  Third-party MCP tool servers can no longer inject policy-conflicting
  instructions through their `description` strings.
- Tool names themselves are defensively sanitized at render time:
  ASCII control characters (including `\n`, `\r`, `\t`), Unicode
  LINE SEPARATOR (`U+2028`), and PARAGRAPH SEPARATOR (`U+2029`) are
  replaced with `?`; backticks are replaced with `'` so the wrapping
  code span cannot be escaped. MCP enforces no charset on tool names,
  so this is a defensive measure against future third-party servers.
- The shell-capability paragraph is gated on a trusted
  `ToolRegistry::bash_available()` signal sourced from the embedder's
  `tool-bash`-marker endpoint configuration — not from matching tool
  names like `"run"`, which a third-party tool could spoof.

## v0.13.0 — /changelog viewer + auto-install all binaries (2026-05-14)

Two threads ship together:

1. **`/changelog` opens an in-TUI viewer.** A new built-in plugin
   (`internal:changelog`) fetches
   `https://raw.githubusercontent.com/robhicks/savvagent-rs/master/CHANGELOG.md`
   and renders it through `tui-markdown` on a dedicated screen, so users
   can read release history without leaving the TUI. Keybindings:
   `j`/`k` and `↑`/`↓` for line scroll, `PageUp`/`PageDown`,
   `g`/`G` for page-top/bottom, `r` to retry on fetch failure,
   `Esc`/`q` to close. No auto-open after self-update — it's an
   on-demand command. (`tui-markdown` is pinned to `0.3` with
   `default-features = false` to skip the `highlight-code` stack —
   syntect + ansi-to-tui — that the changelog doesn't need.)
2. **`/update` now swaps every binary in the release archive, and
   does it automatically on launch.** The v0.12.1 "Known limitations"
   are resolved: previously `/update` only replaced the main
   `savvagent` binary, leaving the six helpers
   (`savvagent-anthropic`, `savvagent-gemini`, `savvagent-openai`,
   `savvagent-tool-fs`, `savvagent-tool-bash`, `savvagent-tool-grep`)
   pinned to whatever version was already on disk. v0.13.0 introduces
   a `CargoDistInstaller` that pipes the per-release
   `savvagent-installer.sh` (or `.ps1` on Windows) through the shell,
   replacing every binary in the archive via the same trusted path
   used for fresh installs. Update detection is also no longer
   gated on user action: the TUI runs the install in the background
   on startup and `/update` is now a retry/force-now command rather
   than the primary trigger. Two new `UpdateState` variants —
   `Installing` and `InstallFailed` — track in-flight installs and
   surface failures in the status banner.

### Added

- `internal:changelog` built-in plugin and `/changelog` slash command
  (PR #70, closes #68). Locale strings added in en/es/hi/pt.

### Changed

- `internal:self-update` runs the cargo-dist installer on startup so
  every binary in the release archive is replaced on the next
  restart. `/update` becomes a retry/force-now affordance. (PR #69,
  closes #67.) New `UpdateState::Installing` and
  `UpdateState::InstallFailed` entries cover the new lifecycle.

### Upgrade notes

- Users running v0.11.0, v0.12.0, or v0.12.1 must re-run the install
  script once to land on v0.13.0 — those releases shipped an
  incorrect `bin_path_in_archive` and cannot self-upgrade. From
  v0.13.0 onwards the auto-install path takes over and no manual
  re-runs are needed.

## v0.12.1 — self-update cache hygiene (2026-05-13)

### Fixed

- `internal:self-update` test suite no longer poisons the developer's
  real `~/.savvagent/update-check.json`. Two `#[tokio::test]` cases
  (`host_starting_spawns_check_that_updates_state`,
  `other_events_are_ignored`) previously invoked the plugin's
  production `on_event` path with a stub fetcher returning
  `v99.99.99`; that path resolved the cache file via the live `$HOME`
  and wrote the stub tag to disk, which the installed binary then
  served for the full 24h TTL — causing `/update` to advertise
  `v99.99.99` and 404 on download. The plugin now accepts a test-only
  `cache_path_override`, the affected tests redirect to a tempdir,
  and a positive regression assertion verifies the override is wired
  through to `cache::save`. Field installs were unaffected — only
  contributors who ran `cargo test` saw the symptom. If your local
  `/update` is wedged on `v99.99.99`, delete
  `~/.savvagent/update-check.json` once and relaunch.

## v0.12.1 — `/update` fixes (2026-05-13)

### Fixed

- **`/update` could not find the binary inside the release archive.**
  v0.11.0 and v0.12.0 configured `self_update` with the default
  `bin_path_in_archive` (`{{ bin }}`, archive root), but cargo-dist
  nests every Unix binary under a top-level `savvagent-{target}/`
  directory in the tarball. The apply path therefore failed on
  Linux/macOS with
  `Could not find the required path in the archive: "savvagent"`.
  `bin_path_in_archive` is now set to `savvagent-{{ target }}/{{ bin }}`
  on Unix and `{{ bin }}` on Windows (the Windows zip ships flat). The
  fix takes effect for users running v0.12.1 or later; v0.11.0 and
  v0.12.0 binaries already in the field cannot self-upgrade through
  this bug — re-run the install script
  (`curl -LsSf https://github.com/robhicks/savvagent-rs/releases/latest/download/savvagent-installer.sh | sh`)
  to get to v0.12.1 the first time.
- **Test suite no longer poisons the developer's real
  `~/.savvagent/update-check.json`.** Two `#[tokio::test]` cases in
  `internal:self-update` invoked the production `on_event` path with
  a stub fetcher returning `v99.99.99`. That path resolved the cache
  file via the live `$HOME` and persisted the stub tag to disk, so an
  installed binary launched after `cargo test` would see `v99.99.99`
  as the latest release for the full 24h TTL and fail download with
  a 404 (the tag doesn't exist on GitHub). The plugin now accepts a
  test-only `cache_path_override` and a positive regression assertion
  confirms the override is wired through to `cache::save`. Field
  installs were unaffected — only contributors who ran the suite saw
  the symptom. If your local `/update` is wedged on `v99.99.99`,
  delete `~/.savvagent/update-check.json` once and relaunch.

### Known limitations (not fixed in 0.12.1)

- `/update` only swaps the main `savvagent` binary; the six helper
  binaries (`savvagent-anthropic`, `savvagent-gemini`,
  `savvagent-openai`, `savvagent-tool-fs`, `savvagent-tool-bash`,
  `savvagent-tool-grep`) shipped in the same archive stay at the
  prior version on disk. They still work because SPP and the MCP tool
  protocol are stable across patch boundaries, but a future
  release will broaden the swap to cover the full archive.

## v0.12.0 — Gemini polish, model picker, TUI integrity (2026-05-13)

Five threads ship together:

1. **Gemini connectivity fixed end-to-end.** Tool schemas (JSON Schema
   from `schemars`) are now sanitized into the OpenAPI subset Gemini's
   protobuf parser accepts, and the retired `gemini-1.5-flash` default
   is replaced with `gemini-2.5-flash`. Gemini also gains `list_models`
   support so the new picker works there.
2. **/model is an interactive picker.** No-args `/model` opens a list
   of the active provider's models with ↑/↓ navigation and Enter to
   switch. The choice is persisted per provider to
   `~/.savvagent/models.toml` and re-applied on reconnect.
3. **TUI rendering integrity.** Tool subprocesses (`tool-fs`,
   `tool-bash`, `tool-grep`) and the host's own tracing no longer
   bleed onto ratatui's alternate screen. All `tracing` output now
   lands in `~/.savvagent/logs/`.
4. **Keybindings help modals.** New `/prompt-keybindings` and
   `/editor-keybindings` slash commands open scrollable, sectioned
   help screens (chrome shared via a new `keybindings_view` module).
5. **Editor syntax theme.** `view-file` / `edit-file` now syntax-color
   code using a theme derived from the active TUI palette — switching
   themes re-themes the editor.

### New features

- `/model` picker — `Effect::SetActiveModel` and
  `ScreenArgs::ModelPicker` added to the plugin contract. The
  `internal:model` plugin owns the picker screen and emits
  `Effect::OpenScreen` for no-args invocations; typed-arg
  invocations (`/model <id>`) still apply directly.
- Per-provider model persistence at `~/.savvagent/models.toml`
  (`schema_version = 1`). Precedence on connect: `SAVVAGENT_MODEL` env
  var > persisted file > provider default.
- Gemini `list_models` — queries `v1beta/models`, filters to entries
  whose `supportedGenerationMethods` includes `generateContent`, and
  surfaces `gemini-2.5-flash` as the default when present.
- `/prompt-keybindings` — modal listing the keybindings active in the
  main prompt input.
- `/editor-keybindings` — modal listing the ratatui-code-editor
  keybindings active in `view-file` / `edit-file`.

### Fixes

- **Gemini tool schemas** — a new sanitizer
  (`provider-gemini::schema`) rewrites incoming JSON Schemas into
  Gemini's OpenAPI subset before they hit the wire. It inlines
  `$ref`/`$defs`, drops `$schema`/`$id`/`$comment`/
  `additionalProperties`/`unevaluatedProperties`/`patternProperties`,
  converts `type: ["X", "null"]` to `type: "X"` + `nullable: true`,
  rewrites `const: X` as `enum: [X]`, renames `oneOf` to `anyOf`, and
  lifts bare `{"type": "null"}` members out of `anyOf` (collapsing
  single-remaining-member `anyOf` into the parent). Resolves the
  `Unknown name "$schema"` / `Cannot find field "const"` /
  `Proto field is not repeating, cannot start list` cascades that
  previously made every Gemini turn fail at request validation.
- **Default Gemini model** bumped to `gemini-2.5-flash`; the
  retired `gemini-1.5-flash` was returning `ModelNotFound`.
- **TUI alt-screen integrity** — each tool subprocess's stderr is
  redirected to `~/.savvagent/logs/tools/<binary>.log` after sandbox
  wrapping; the host's own tracing writes to
  `~/.savvagent/logs/savvagent.log`. Tool crates' default log level
  dropped from `info` to `warn`. `RUST_LOG` still overrides for
  debugging.

### Plugin SDK changes

- `Effect::SetActiveModel { id, persist }` — runtime resolves the
  active provider, rebuilds its in-process host with `id`, and
  optionally writes to `~/.savvagent/models.toml`.
- `ScreenArgs::ModelPicker { current_id, models: Vec<ModelEntry> }`
  — picker args; `apply_effects::open_screen` patches the variant
  from `App::cached_models` (refreshed after every connect and
  model change).
- `ModelEntry { id, display_name }` — new type, exported from the
  plugin crate root.

### Dependencies

No new external dependencies.

## v0.11.0 — TUI Self Update (2026-05-13)

In-band self-update. On launch the TUI asynchronously checks the GitHub
Releases API for `robhicks/savvagent-rs` and, if a newer release is
available, surfaces a one-line banner above the existing tips row. A new
`/update` slash command downloads the matching cargo-dist tarball for
the running target triple and atomically replaces the running binary.

### New features

- `home.banner` slot — new render slot above `home.tips`. The plugin
  paints "Update available: vX → vY  (run /update)" when a newer
  release is detected, or "Updated to vY. Restart savvagent to apply."
  after a successful `/update`. Empty/blank when there is no update.
- `/update` — download and install the latest release. Two
  `Effect::PushNote` notes are emitted: a "Downloading vY…" line, then
  a success or failure line. On failure the banner stays in the
  "Update available" state so the user can retry.
- 24-hour cache for the version check, persisted to
  `~/.savvagent/update-check.json`. Subsequent launches within the TTL
  skip the network call entirely.
- Opt-out: `SAVVAGENT_NO_UPDATE_CHECK=1` env var or `--no-update-check`
  CLI flag. Either signal disables both the check and `/update`.
- Dev builds (binary running from `target/{debug,release}/`) are
  detected automatically and short-circuit to `UpdateState::Disabled`
  with no network call.
- On-quit stderr hint: after `/update` succeeds, the TUI prints a
  one-liner to stderr after the alt-screen tears down, so the user
  sees "savvagent: installed v0.11.0 (was v0.10.0). Restart to use
  the new version." even after closing the TUI.

### Plugin SDK changes

None — the feature is implemented entirely inside the savvagent crate
as a new built-in plugin (`internal:self-update`). The `Plugin` trait
surface is unchanged.

### Release infrastructure

- `cargo-dist` `unix-archive` switched from `.tar.xz` to `.tar.gz` so
  the `self_update` crate can extract Unix release artifacts using
  gzip support shipped with the crate (xz extraction would require an
  additional native dependency). v0.10.x users upgrade by re-running
  the curl|sh installer once; from v0.11.0 onwards `/update` handles
  subsequent upgrades.

### Dependencies

- `semver = "1"` — version comparisons in `internal:self-update`.
- `self_update = "=0.42.0"` — atomic binary replacement. Pinned to
  0.42 because 0.43.1 has type-inference failures against rustc 1.85
  + edition 2024.

### Known limitations

- The actual binary swap is not unit-tested end-to-end — the plugin's
  orchestration is covered by a `BinarySwapper` stub, but the
  production `self_update::backends::github::Update` path requires a
  real release artifact to exercise. End-to-end verification happens
  post-v0.11.0 once a v0.11.x release exists to update to.
- `~/.savvagent/config.toml` opt-out (mentioned in the original issue)
  is deferred. Env var + CLI flag are sufficient for v0.11.0;
  introducing a config.toml for one boolean was over-scope.
- `cargo install --force savvagent` is not supported — savvagent is
  not yet published to crates.io. cargo-dist tarballs are the only
  distribution channel.

### Migration notes

No external API or wire-protocol changes. Existing v0.10.x users
upgrade by re-running the curl|sh installer. Plugin authors are
unaffected.

## v0.10.1 — TUI polish (2026-05-13)

Render-path fixes for theme legibility, layout breathing room, and
command-palette alignment. No API, plugin, or wire-format changes.

### Fixes

- TUI padding: removed the outer terminal-edge inset and added
  interior padding to each bordered widget (header, conversation
  log, input, popups, screen-stack modals). Content now sits inside
  the borders with breathing room instead of the entire app being
  inset from the terminal edges.
- Block titles ("Conversation", popup titles, screen-stack modal
  titles) now render in `palette.fg` instead of inheriting the
  border color, so they stay legible on upstream themes whose
  `border`/`selection` color is a pale chrome accent.
- Upstream themes' `muted` color is now blended 50% toward `fg`,
  so command descriptions, footer chrome, and conversation notes
  remain readable across Solarized Light, Catppuccin Latte, Tokyo
  Night Day, etc. Built-in themes are unchanged.
- Command palette: description column aligns across rows even when
  command names exceed 12 characters (`/connect anthropic`,
  `/connect gemini`, …). Width is now computed from the longest
  filtered name with a 12-char floor and 2-col gutter.
- Footer right slot widened from 33% to 50% so the working-directory
  path and version string no longer clip the SemVer patch level
  (e.g., `v0.10.0` rendering as `v0.10.`).

## v0.10.0 — Localize TUI (2026-05-13)

Internationalization for the TUI. The savvagent crate now ships
`rust-i18n` catalogs for English, Spanish, Portuguese, and Hindi. A
new `internal:language` built-in plugin contributes a `/language`
slash command and a centered-modal picker (mirroring the existing
`internal:themes` plugin).

### New features

- `/language` — open the language picker. Arrow-key navigation,
  type-to-filter, Enter to apply + persist, Esc to cancel.
- `/language <code>` — directly switch to a supported locale
  (`en`, `es`, `pt`, `hi`).
- Boot-time locale detection: `~/.savvagent/language.toml` > `LC_ALL`
  > `LC_MESSAGES` > `LANG` > `en`.
- Live preview during picker navigation; Esc reverts.

### Plugin SDK changes

- `Effect::SetActiveLocale { code, persist }` — additive variant on
  the `#[non_exhaustive]` Effect enum.
- `ScreenArgs::LanguagePicker { current_code }` — additive variant on
  the `#[non_exhaustive]` ScreenArgs enum.
- `savvagent-plugin` crate version: 0.9.0 → 0.10.0.

### Known limitations

- Slash-command summaries in the command palette are captured at the
  boot locale; changing language mid-session requires a restart to
  refresh the summary column. Modal titles, picker rows, status-bar
  text, and pushed notes all re-resolve every frame.
- Hindi rendering depends on a terminal font that includes Devanagari
  glyphs. Without one, rows fall back to replacement boxes. The
  language code column (`hi`) is always ASCII, so the user can still
  filter and select.
- `LANGUAGE=` (glibc compound-locale env var) is not honored — only
  POSIX `LC_ALL` / `LC_MESSAGES` / `LANG`.
- A small number of user-facing literals were deferred during PR 6's
  string sweep (ui.rs header, plugin manifest `name` fields, a handful
  of `app.rs` notes). The catalog parity test continues to enforce
  structural correctness on what IS in the catalog; these will be
  cleaned up in a follow-up.

### Migration notes

No external API changes for non-plugin consumers. Plugin authors:
`SetActiveLocale` and `LanguagePicker` are additive on
`#[non_exhaustive]` enums; existing match arms continue to compile
unchanged. The runtime applies `SetActiveLocale` automatically; no
plugin code needs to call `rust_i18n::set_locale` directly.

## [0.9.0] - 2026-05-12

### v0.9.0 — Plugin system

The TUI and host are now routed through a typed `Plugin` trait. Eighteen
built-in plugins compose the entire UI surface — chrome (footer, tips),
splash, command palette, modal screens (themes, plugins manager, connect,
resume, file viewer/editor), slash commands (`/clear`, `/save`, `/model`,
`/connect`, `/resume`, `/quit`), and provider plugins for Anthropic,
OpenAI, Gemini, and the local Ollama backend. A new screen-stack runtime
replaces the v0.8 `InputMode` state machine with consistent open / close /
back semantics across every modal. Plugin enable / disable state persists
to `~/.savvagent/plugins.toml`. The trait surface is intentionally
WIT-portable so the same plugin contract can drive a future WASM
Component-Model loader without churn.

### Plugin runtime (`savvagent-plugin` crate)

- **New leaf crate `savvagent-plugin`** carrying owned types + trait
  definitions only. No `&str` returns, no callbacks, no host references
  — every method takes / returns owned data so the same contract can
  cross a WASM Component-Model boundary verbatim.
- **`Plugin` trait surface:** `manifest()`, `handle_slash()`,
  `on_event()`, `render_slot()`, and `create_screen()`. A plugin
  implements only the methods relevant to its contributions; defaults
  return `Vec::new()` / `None`.
- **Effect enum** (`Effect`) describes every action a plugin can request:
  `PushNote`, `OpenScreen`, `CloseScreen`, `Stack`, `RunSlash`,
  `SetActiveTheme`, `RegisterProvider`, `SaveTranscript`, `ClearLog`,
  `TogglePlugin`, `Quit`, `PrefillInput`. The host applies effects in
  the order returned; `Stack` composes effects without per-plugin
  recursion.
- **Concrete enums for everything that crosses the boundary** —
  `PluginKind`, `Slot`, `ScreenLayout`, `ThemeColor`, `HostEvent`,
  `Effect` — instead of `Box<dyn Trait>` or string tags. WIT export is
  mechanical: no design work pending.

### Screen stack replaces `InputMode`

- The v0.8 `InputMode` enum (Normal, SelectingTheme, ConnectPicker, …)
  is gone. A single `screen_stack: Vec<ActiveScreen>` field on `App`
  now holds whichever screens are open; the textarea is the focus
  target when the stack is empty.
- **`ScreenLayout` variants** — `CenteredModal { width, height }`,
  `Fullscreen`, `BottomSheet { height }` — clear and repaint their
  region every frame so modals never bleed through onto each other.
- **Open / close / back semantics are uniform.** Esc pops the top
  screen, Enter commits, Ctrl-C closes everything. Splash, command
  palette, themes picker, plugins manager, connect picker, resume
  picker, and the file viewer / editor all participate.

### 18 built-in plugins

Grouped by category. Every entry is shipped enabled by default unless
noted; Core plugins cannot be disabled.

- **Chrome (Core):**
  - `internal:home-footer` — three-segment status bar (provider
    badges left, turn state center, `working_dir · ~N ctx · $0.00 · vX.Y.Z`
    right).
  - `internal:home-tips` — bottom-of-screen muted hint line.
- **Splash (Core):** `internal:splash` — startup splash screen.
- **Modals (Core):**
  - `internal:command-palette` — Ctrl-P / `/` palette.
  - `internal:themes` — `/theme` picker (replaces the v0.8 dedicated
    modal; now a `Screen` plugin like everything else).
  - `internal:plugins-manager` — `/plugins` enable / disable manager.
- **File screens (Optional):**
  - `internal:view-file` — `/view` centered modal.
  - `internal:edit-file` — `/edit` centered modal.
- **Slash-command plugins:**
  - `internal:clear` (Core), `internal:save` (Core), `internal:model`
    (Core), `internal:connect` (Core), `internal:resume` (Core),
    `internal:quit` (Core).
- **Provider plugins (Optional):**
  - `internal:provider-anthropic`, `internal:provider-openai`,
    `internal:provider-gemini`, `internal:provider-local`.

### Plugin manager + persistence

- **`/plugins` opens the manager modal** listing every plugin with its
  kind, version, contribution summary (which slash commands / slots /
  screens it owns), and an on / off toggle. Core plugins render greyed
  out — selecting them is a no-op.
- **Toggles persist atomically** to `~/.savvagent/plugins.toml`
  (schema `version = 1`). Writes go through a tempfile + rename so a
  crash mid-write never leaves a half-baked file.
- **File permissions:** 0o600 on the file, 0o700 on the
  `~/.savvagent/` directory (Unix). Missing file = all defaults
  (additive — never a hard failure on first launch).
- **Startup re-applies persisted overrides** before building the slash
  / slot / screen indexes, so a disabled plugin contributes nothing
  from frame one.

### Theme system (v0.8 work preserved + extended)

- **All v0.8 themes still present** — the 3 hand-rolled built-ins
  (`default`, `dark-mono`, `pastel`) plus the 15 upstream
  `ratatui-themes` slugs (`dracula`, `nord`, `tokyo-night`,
  `catppuccin-mocha`, `catppuccin-latte`, `gruvbox-dark`,
  `gruvbox-light`, `solarized-dark`, `solarized-light`,
  `one-dark-pro`, `monokai-pro`, `rose-pine`, `kanagawa`,
  `everforest`, `cyberpunk`).
- **Picker is now a `Screen` plugin** (`internal:themes`) — same
  open / close semantics as every other modal. `/theme` opens the
  picker; `/theme <slug>` still applies + persists directly without
  opening the modal.
- **Semantic `ThemeColor` variants** — `Fg`, `Bg`, `Accent`, `Muted`,
  `Error`, `Warning`, `Success`, `Secondary`, `Border` — so plugin
  chrome resolves through the active theme rather than hard-coding
  Crossterm colors. Switch themes and every plugin's rendering
  follows.

### Event-hook dispatch

- **`HostEvent` lifecycle events** flow through a `HookDispatcher`:
  `HostStarting`, `Connect`, `Disconnect`, `TurnStart`, `TurnEnd`,
  `ToolCallStart`, `ToolCallEnd`, `PromptSubmitted`, `TranscriptSaved`,
  `ProviderRegistered`, `ContextSizeChanged`.
- **Per-subscriber error isolation.** A panicking or errant plugin
  doesn't take down the dispatcher or other subscribers; its error is
  logged + skipped.
- **Shared `MAX_DISPATCH_DEPTH` cap** with `RunSlash` re-entry — a
  plugin that emits `RunSlash` in response to an event still respects
  the same recursion limit as plugin-emitted slash dispatch, so
  feedback loops fail fast.

### Multi-region home layout

- **Three-segment footer** below the textarea:
  - **Left:** provider badges, contributed by whichever provider
    plugins are enabled + connected.
  - **Center:** turn state ("ready", "thinking…", tool-call summary),
    contributed by `home-footer`.
  - **Right:** `working_dir · ~N ctx · $0.00 · vX.Y.Z`, contributed by
    `home-footer`.
- **1-row vertical / 2-col horizontal frame margin** around the content
  area for breathing room.
- **`$0.00` is a placeholder** — see _Out of scope_ below for the
  deferred cost-tracking work.

### UX polish (v0.9 hotfix, shipped pre-tag)

These landed on master between PR 8 and the release branch to fix
issues caught during manual smoke-testing:

- **Command palette is driven by the live slash index.** Disabled
  plugins' slashes don't appear; newly added plugins show up without
  touching a static list anywhere.
- **`/view` and `/edit` open as centered modals** (popup, not
  full-bleed) and strip the `@` file-picker prefix from path args so
  `/view @src/main.rs` works as expected.
- **`/edit` and `/view` from the palette prefill the textarea**
  (`/view ` / `/edit ` with a trailing space) so the user can finish
  the path via the `@` file picker before submitting.
- **Ctrl-P opens the palette** (matching v0.8 muscle memory).
- **`/quit` is a plugin again** (`internal:quit`, Core) — restored as
  a first-class plugin contribution rather than an `App::handle_command`
  arm.
- **Disabling a plugin actually disables its slashes.** Legacy
  `App::handle_command` arms for `/clear`, `/save`, `/view`, `/edit`,
  and `/quit` are removed — those commands now route exclusively
  through the slash index, so toggling the owning plugin off removes
  the command from the surface.

### Behavior changes (potentially breaking)

- **New config file `~/.savvagent/plugins.toml`** (schema v1).
  Additive: missing-file = all defaults; existing installs upgrade
  without touching anything on disk until the user toggles something.
- **Splash + theme persistence files unchanged** (`splash.toml`,
  `theme.toml`).
- **`InputMode::SelectingTheme` (and the rest of `InputMode`) deleted.**
  Any out-of-tree consumer reading `App` internals would notice — no
  public API impact otherwise.

### Internal architecture

- **New crate:** `crates/savvagent-plugin/` (leaf, no host deps; only
  WIT-portable types + the `Plugin` / `Screen` traits).
- **Consolidation:** the v0.8 `crates/savvagent/src/{splash, palette,
  theme, providers}.rs` modules collapsed into
  `crates/savvagent/src/plugin/builtin/` per-plugin directories.
- **`App::handle_command` slimmed** to just the legacy `/connect` arm
  (every other slash routes through the slash index now).
- **New runtime modules under `crates/savvagent/src/plugin/`:**
  `registry`, `manifests` (slash / slot / screen indexes), `effects`
  (`apply_effects` + `dispatch_host_event`), `hooks`
  (`HookDispatcher`), `slash`, `keybindings`, `screen_stack`, `slots`,
  `convert`.

### Out of scope (deferred)

These are deliberately not in v0.9 and have follow-up issues:

- WASM Component-Model loader / `.wit` file (the trait surface is
  WIT-portable; no loader yet).
- Third-party plugin discovery + signing.
- Sidebar UI.
- Streaming-delta hooks (per-token `on_text_delta` etc.).
- Hot-reload of disabled plugins (toggle takes effect on next launch
  for some surfaces).
- Real session-wide token-usage tracking + cost. The `$0.00` in the
  status line is a placeholder until `TurnOutcome.usage` accumulation
  and a per-model pricing table land.
- `HostEvent::Disconnect` emission — the variant exists and dispatches
  correctly, but no current code path fires it.
- Ollama health-check before `/connect local` — currently builds the
  client unconditionally; PR 7 punted the check to a follow-up.

[0.9.0]: https://github.com/savvagent/savvagent-cli/releases/tag/v0.9.0
