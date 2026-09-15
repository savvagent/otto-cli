//! Otto TUI entry point.
//!
//! Bootstraps a [`otto_host::Host`] in one of two ways:
//!
//! 1. **In-process (default).** Each provider crate is linked as a library;
//!    the TUI builds a [`ProviderHandler`](otto_mcp::ProviderHandler) and
//!    wraps it in `InProcessProviderClient` — no MCP transport, no spawned
//!    binary. The TUI scans the OS keyring for a saved API key and
//!    auto-connects to the first provider it finds; otherwise the user
//!    runs `/connect`.
//! 2. **Remote MCP (opt-in).** If `OTTO_PROVIDER_URL` is set the TUI
//!    connects to that Streamable HTTP MCP server instead — useful for
//!    pointing at a long-running `otto-anthropic`/`otto-gemini`
//!    binary or a third-party MCP provider.
//!
//! Other configuration:
//!
//! - `OTTO_MODEL`          (overrides the per-provider default)
//! - `OTTO_TOOL_FS_BIN`    (default `otto-tool-fs` on $PATH)
//! - `OTTO_TOOL_BASH_BIN`  (default `otto-tool-bash` on $PATH)
//! - `OTTO_TOOL_GREP_BIN`  (default `otto-tool-grep` on $PATH)
//! - `OTTO_TOOL_LSP_BIN`   (default `otto-tool-lsp` on $PATH)
//! - `OTTO_TOOL_WEB_BIN`   (default `otto-tool-web` on $PATH)

#![allow(clippy::collapsible_if)] // pre-existing debt; many sites under rustc 1.95 new lint

rust_i18n::i18n!("locales", fallback = "en");

#[cfg(test)]
mod i18n_smoke {
    #[test]
    fn smoke_key_resolves_in_en() {
        rust_i18n::set_locale("en");
        assert_eq!(rust_i18n::t!("smoke.hello"), "hello, world");
    }
}

mod app;
mod canvas_input;
mod config_file;
mod creds;
mod mcp_config_writer;
mod mcp_oauth;
mod migration;
mod models_pref;
mod palette;
mod plugin;
mod prompt_history;
mod providers;
mod routing_pref;
mod splash;
#[cfg(test)]
mod test_helpers;
mod tui;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use app::{
    App, BashCommandError, Entry, InputMode, collect_transcript_entries, make_input_textarea,
    parse_bash_command,
};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use otto_host::{
    BashNetworkChoice, Host, HostConfig, HttpAuth, LegacyModelResolution, PermissionDecision,
    ProviderEndpoint, ProviderRegistration, ProviderView, SandboxConfig, SandboxMode,
    ToolCallStatus, ToolEndpoint, TranscriptError, TurnEvent, resolve_legacy_model,
};
use providers::{ProviderSpec, effective_providers};
use tokio::sync::{RwLock, mpsc};

/// Wrapped-line step for one PageUp/PageDown press on the conversation log.
/// Matches the increment used by the keybindings-help screen so scroll
/// pacing feels consistent across modals.
const LOG_SCROLL_STEP: u16 = 10;

/// Wrapped-line step for one mouse-wheel notch. Smaller than the PageUp/Down
/// step so wheel scrolling feels granular — most terminals emit a notch per
/// physical detent, which adds up fast.
const MOUSE_WHEEL_SCROLL_STEP: u16 = 3;

fn global_quit_allowed(top_screen_id: Option<&str>) -> bool {
    top_screen_id != Some("splash")
}

fn should_route_home_keybinding(key: &crossterm::event::KeyEvent, prompt: &[String]) -> bool {
    !matches!(
        key.code,
        KeyCode::Char('/') if key.modifiers.is_empty() && prompt.iter().any(|line| !line.is_empty())
    )
}

fn connect_rejected_note_key(
    kind: otto_protocol::ErrorKind,
    api_key_required: bool,
) -> &'static str {
    if api_key_required && kind == otto_protocol::ErrorKind::Authentication {
        "notes.connect-rejected-keyed"
    } else {
        "notes.connect-failed"
    }
}

/// Worker → main-loop messages.
pub(crate) enum WorkerMsg {
    Event(TurnEvent),
    /// Sent if `run_turn_streaming` returned an error.
    Error(String),
    /// Sent if `run_turn_streaming` returned an authentication error from a
    /// provider, so the main loop can additionally render a recovery hint.
    TurnAuthError {
        message: String,
        provider_display_name: String,
    },
    /// Sent when a `/bash` direct-invocation worker finishes (success or
    /// error). The main loop uses this to clear `app.is_loading`, mirroring
    /// the `TurnComplete` path for model-driven turns.
    BashDone,
    /// Sent when a `/disconnect` drain/force worker completes successfully.
    DisconnectCompleted {
        provider: String,
        mode: String,
    },
    /// Sent when a `/disconnect` worker encounters an error from
    /// `Host::remove_provider`.
    DisconnectFailed {
        provider: String,
        err: String,
    },
    /// Sent by the turn worker after it restores the host's model back to
    /// `original` following a one-turn `next_turn_model_override`. The main
    /// loop syncs `app.model` so the status bar reflects the restored value.
    ModelRestored(String),
}

pub(crate) type HostSlot = Arc<RwLock<Option<Arc<Host>>>>;

/// Resolved paths for every bundled tool-server binary the TUI knows how to
/// register. Each field is `None` when the binary couldn't be found; the
/// host just doesn't advertise that tool's surface in `tools/list`.
#[derive(Clone, Default)]
pub(crate) struct ToolBins {
    fs: Option<PathBuf>,
    bash: Option<PathBuf>,
    grep: Option<PathBuf>,
    lsp: Option<PathBuf>,
    web: Option<PathBuf>,
}

impl ToolBins {
    /// Append every populated entry as a stdio [`ToolEndpoint`] on `config`.
    fn apply(&self, mut config: HostConfig) -> HostConfig {
        for (name, path) in [
            ("fs", self.fs.as_deref()),
            ("bash", self.bash.as_deref()),
            ("grep", self.grep.as_deref()),
            ("lsp", self.lsp.as_deref()),
            ("web", self.web.as_deref()),
        ]
        .into_iter()
        .filter_map(|(name, path)| path.map(|p| (name, p)))
        {
            config = config.with_tool(ToolEndpoint::Stdio {
                name: name.to_string(),
                command: path.to_path_buf(),
                args: vec![],
                env: Default::default(),
            });
        }
        config
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let (mut app, host_slot, project_root, tool_bins) = bootstrap_app_and_host().await?;

    let mut terminal = tui::init()?;

    // Restore terminal on panic.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = tui::restore();
        original_hook(info);
    }));

    let res = run_app(
        &mut terminal,
        &mut app,
        host_slot.clone(),
        project_root,
        tool_bins,
    )
    .await;

    let _ = tui::restore();

    if let Some(host) = current_host(&host_slot).await {
        if let Err(e) = save_transcript_now(&app, &host).await {
            eprintln!("warning: could not save transcript on exit: {e}");
        }
    }
    let host = {
        let mut slot = host_slot.write().await;
        slot.take()
    };
    if let Some(host) = host {
        host.shutdown().await;
    }

    if let Err(err) = res {
        eprintln!("{err:?}");
    }

    // If `/update` succeeded during this session, the on-disk binary is
    // a newer version than the one we're still running. Surface a hint
    // on stderr now that the alt-screen has torn down.
    if let Some((from, to)) = plugin::builtin::self_update::pending_restart_hint() {
        eprintln!("otto: installed v{to} (was v{from}). Restart to use the new version.");
    }

    Ok(())
}

/// Result of building the provider-pool host, independent of `App`
/// construction — the value [`build_app_with_host`] takes to construct
/// `App` on top of an already-built host.
pub(crate) struct HostBoot {
    /// The started provider-pool host, if startup connected successfully.
    pub host: Option<Arc<Host>>,
    /// Model id to show in the header for the initial active provider.
    pub header_model: String,
    /// `&'static` id of the initial active provider, if one connected.
    pub provider_id: Option<&'static str>,
    /// One-shot startup notes (timeouts, build failures, routing parse errors)
    /// to surface once `App` exists.
    pub startup_notes: Vec<String>,
    /// Seed data for the `/mcp` manager screen.
    pub mcp_manager_seed: McpManagerSeed,
    /// Mirrors `config_file.startup.verbose` at the moment the host was
    /// built. Carried forward onto `App` so the async pool-add drain
    /// (`apply_pending_pool_add`) can apply the same startup-quiet gate
    /// after `ConfigFile` itself has gone out of scope.
    pub startup_verbose: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct McpManagerSeed {
    pub configured: Vec<McpServerSummary>,
    pub skip_notes: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum McpServerAuthSummary {
    #[default]
    None,
    Bearer,
    Oauth,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct McpServerSummary {
    pub name: String,
    pub transport: &'static str,
    pub target: String,
    pub auth: McpServerAuthSummary,
}

fn summarize_mcp_server(entry: &config_file::McpServerEntry) -> McpServerSummary {
    match entry {
        config_file::McpServerEntry::Stdio { name, command, .. } => McpServerSummary {
            name: name.clone(),
            transport: "stdio",
            target: command.clone(),
            auth: McpServerAuthSummary::None,
        },
        config_file::McpServerEntry::Http {
            name, url, auth, ..
        } => McpServerSummary {
            name: name.clone(),
            transport: "http",
            target: url.clone(),
            auth: match auth {
                config_file::McpAuthMode::None => McpServerAuthSummary::None,
                config_file::McpAuthMode::Bearer => McpServerAuthSummary::Bearer,
                config_file::McpAuthMode::Oauth => McpServerAuthSummary::Oauth,
            },
        },
    }
}

pub(crate) fn build_disconnected_mcp_manager_seed(
    config_file: &config_file::ConfigFile,
) -> McpManagerSeed {
    let mut skip_notes = Vec::new();
    let mut seen_names = std::collections::HashSet::new();
    let mut configured = Vec::new();

    for entry in &config_file.mcp_servers {
        let summary = summarize_mcp_server(entry);
        let name = summary.name.clone();
        configured.push(summary);

        if let Err(reason) = entry.validate(&seen_names) {
            skip_notes.push((name, reason));
            continue;
        }
        seen_names.insert(name.clone());

        match entry {
            config_file::McpServerEntry::Stdio { .. }
            | config_file::McpServerEntry::Http {
                auth: config_file::McpAuthMode::None,
                ..
            } => {}
            config_file::McpServerEntry::Http {
                auth: config_file::McpAuthMode::Bearer,
                ..
            } => match crate::creds::mcp_load(&name) {
                Ok(Some(secret)) if !secret.trim().is_empty() => {}
                Ok(_) => {
                    skip_notes.push((name, "missing keyring secret for mcp bearer auth".into()))
                }
                Err(err) => {
                    skip_notes.push((name, format!("failed to read keyring secret: {err}")))
                }
            },
            config_file::McpServerEntry::Http {
                auth: config_file::McpAuthMode::Oauth,
                ..
            } => match crate::mcp_oauth::load_stored_secret(&name) {
                Ok(Some(secret)) => {
                    if let Err(reason) = secret.validate_for_startup() {
                        skip_notes.push((name, reason));
                    }
                }
                Ok(None) => skip_notes.push((
                    name,
                    "oauth authorization required; open /mcp to authorize".into(),
                )),
                Err(err) => skip_notes.push((name, err)),
            },
        }
    }

    McpManagerSeed {
        configured,
        skip_notes,
    }
}

/// Resolve the bundled tool binaries. Cheap, local, and `Send`.
pub(crate) fn build_tool_bins() -> ToolBins {
    ToolBins {
        fs: locate_bundled_bin("otto-tool-fs", "OTTO_TOOL_FS_BIN"),
        bash: locate_bundled_bin("otto-tool-bash", "OTTO_TOOL_BASH_BIN"),
        grep: locate_bundled_bin("otto-tool-grep", "OTTO_TOOL_GREP_BIN"),
        lsp: locate_bundled_bin("otto-tool-lsp", "OTTO_TOOL_LSP_BIN"),
        web: locate_bundled_bin("otto-tool-web", "OTTO_TOOL_WEB_BIN"),
    }
}

/// Bootstraps the TUI: build the provider-pool host, then `App` on top of
/// it, awaited sequentially on one task. The ratatui TUI path in `main` is
/// the only caller.
///
/// This used to be split into a `bootstrap_host_only` step, kept separate
/// from [`build_app_with_host`] so the network half could in principle run
/// on a background Tokio worker while `App` was built on the GUI thread of
/// the now-removed egui front-end. With only one front-end left, nothing
/// schedules the two halves apart, so that wrapper added a function without
/// adding a caller and was folded back in here. The capability it named
/// didn't disappear: `bootstrap_pool_host` still returns only `Send` data,
/// so a future off-thread or headless bootstrap is a rewrap away, not a
/// redesign. [`build_app_with_host`] stays separate — see its doc comment.
pub(crate) async fn bootstrap_app_and_host() -> Result<(App, HostSlot, std::path::PathBuf, ToolBins)>
{
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let tool_bins = build_tool_bins();
    let loaded_config =
        config_file::ConfigFile::load_or_default(&config_file::ConfigFile::default_path());
    let mcp_server_diagnostics = loaded_config.mcp_server_diagnostics;
    let config_file = loaded_config.config;
    let initial = bootstrap_pool_host(
        &project_root,
        &tool_bins,
        &config_file,
        mcp_server_diagnostics,
    )
    .await;
    build_app_with_host(initial, project_root, tool_bins).await
}

/// Construct `App` from an already-built [`HostBoot`], install the plugin
/// runtime, and wrap the host in a `HostSlot`. Kept as its own function,
/// taking the host as a parameter instead of building it, because that
/// injection point is a real phase boundary: local, in-process app/plugin
/// startup versus the network-bearing `bootstrap_pool_host` that builds the
/// host — `App` itself is `Send` today, so this isn't a `Send`/`!Send` split
/// (that justification left with egui). A test exploits the seam — it hands
/// this a synthetic `HostBoot` with `host: None` to exercise startup's
/// app-construction and plugin-registration behavior without touching the
/// network — see `build_app_startup_skips_conflicting_user_exit_command`.
pub(crate) async fn build_app_with_host(
    initial: HostBoot,
    project_root: std::path::PathBuf,
    tool_bins: ToolBins,
) -> Result<(App, HostSlot, std::path::PathBuf, ToolBins)> {
    let header_model = initial.header_model.clone();
    let initial_provider = initial.provider_id;
    let startup_notes = initial.startup_notes.clone();
    let mcp_manager_seed = initial.mcp_manager_seed.clone();
    let startup_verbose = initial.startup_verbose;

    let host_slot: HostSlot = Arc::new(RwLock::new(initial.host));
    let mcp_statuses = if let Some(host) = current_host(&host_slot).await {
        host.tool_server_statuses().await
    } else {
        vec![]
    };

    let transcript_dir = transcript_dir();

    // `/language` still controls the same runtime surface; only the
    // persisted on-disk source moved to `~/.otto/config.toml`.
    let initial_locale = crate::plugin::builtin::language::catalog::detect_initial();
    rust_i18n::set_locale(&initial_locale);
    let mut app = App::new(header_model, transcript_dir, initial_locale);
    app.startup_verbose = startup_verbose;
    app.load_prompt_history(&project_root);

    // Load user-defined hooks from settings.json. The HooksIndex is
    // shared with the `internal:user-hooks` plugin via App's
    // user_hooks_index Arc; the plugin reads it under each event
    // dispatch and writes it on `/reload-hooks`.
    {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let initial_idx =
            crate::plugin::builtin::user_hooks::discovery::walk_all(&project_root, &home);
        let mut g = app.user_hooks_index.write().await;
        *g = initial_idx;
    }

    {
        use crate::plugin::manifests::Indexes;
        use crate::plugin::registry::PluginRegistry;
        use otto_plugin::PluginKind;

        // Theme provider shared with wasm static/interactive adapters via
        // their `current-theme()` host import. v0.18.0 hands every wasm
        // plugin an empty snapshot; theme-sync to the TUI's active
        // palette is a Task 13 polish item. The handle is built here so
        // a future revision can update it alongside `Effect::SetTheme`
        // without restructuring the bootstrap.
        let wasm_theme = otto_plugin_wasm::host_imports::theme::provider(Vec::new());
        let home_dir = dirs::home_dir();
        let (set, external_warnings) = plugin::register_builtins_with_external(
            host_slot.clone(),
            app.trust_levels.clone(),
            app.user_hooks_index.clone(),
            app.session_id.clone(),
            project_root.clone(),
            app.transcript_path.clone(),
            mcp_manager_seed,
            mcp_statuses,
            home_dir.as_deref(),
            wasm_theme,
        )
        .await;
        for warn in external_warnings {
            // External-plugin warnings (untrusted / hash-mismatch /
            // instantiation failure) are surfaced as notes so the user
            // can debug skipped plugins from the TUI without leaving
            // for the log file. `tracing::warn!` already fired for the
            // log-only path inside `register_builtins_with_external`.
            app.push_note(warn);
        }
        let mut registry = PluginRegistry::new(set);

        // Apply persisted Optional-plugin enabled state from
        // ~/.otto/plugins.toml so the initial Indexes::build picks
        // up the user's saved choices. Core plugins are always enabled
        // regardless of what the file says; unknown ids are skipped.
        //
        // Both skip branches warn-log so the user can diagnose two
        // otherwise-silent scenarios:
        //   - downgrade: a previously-disabled plugin no longer exists
        //     in this binary.
        //   - hand-edit: someone added a Core plugin to plugins.toml
        //     (which the spec forbids the runtime to honour).
        let persisted = plugin::builtin::plugins_manager::persistence::load();
        for (pid, enabled) in persisted {
            let Some(plugin) = registry.get(&pid) else {
                tracing::warn!(
                    plugin = %pid.as_str(),
                    "plugins.toml: unknown plugin id; entry ignored (downgrade or removed plugin?)"
                );
                continue;
            };
            let kind = plugin.lock().await.manifest().kind;
            match kind {
                PluginKind::Optional => registry.set_enabled(&pid, enabled),
                PluginKind::Core => tracing::warn!(
                    plugin = %pid.as_str(),
                    "plugins.toml: Core plugins cannot be disabled; ignoring entry"
                ),
            }
        }

        // Honour the user's saved Optional-plugin choices for legacy
        // hardcoded subsystems. The startup splash render lives in
        // `mod splash` rather than the `internal:splash` plugin's
        // Screen; gate the hardcoded path on the plugin's enabled
        // state so toggling `splash` in /plugins actually disables it.
        let splash_id = otto_plugin::PluginId::new("internal:splash").expect("valid built-in id");
        if !registry.is_enabled(&splash_id) {
            app.show_splash = false;
        }

        let indexes = Indexes::build(&registry)
            .await
            .unwrap_or_else(|e| panic!("plugin manifest conflict at startup: {e}"));

        // Push plugin-contributed system-prompt segments to the host now that
        // the enabled set is finalised. This must happen before the first turn
        // so the model sees (e.g.) the html-canvas rendering instructions in
        // its system prompt.
        let startup_segments = registry.active_prompt_segments();
        if let Some(host) = current_host(&host_slot).await {
            host.set_prompt_segments(startup_segments);
        }

        app.install_plugin_runtime(registry, indexes);
    }

    app.connected = host_slot.read().await.is_some();
    app.active_provider_id = initial_provider;
    // If we already have a host (e.g. saved-credentials bootstrap), align
    // the splash sandbox indicator with what the host will actually apply.
    // Otherwise it would briefly show the on-disk preference even when the
    // active host was built with a different SandboxConfig override.
    if let Some(host) = current_host(&host_slot).await {
        app.refresh_splash_sandbox_from_host(host.sandbox_config());
    }
    if !app.connected {
        app.push_note(rust_i18n::t!("notes.not-connected-startup").to_string());
    }
    // Surface any startup-timeout notes that were collected before App existed.
    for note in startup_notes {
        app.push_note(note);
    }
    if tool_bins.fs.is_none() {
        app.push_note(rust_i18n::t!("errors.tool-fs-not-found").to_string());
    }
    if tool_bins.bash.is_none() {
        app.push_note(rust_i18n::t!("errors.tool-bash-not-found").to_string());
    }
    if tool_bins.grep.is_none() {
        app.push_note(rust_i18n::t!("errors.tool-grep-not-found").to_string());
    }
    if tool_bins.lsp.is_none() {
        app.push_note(rust_i18n::t!("errors.tool-lsp-not-found").to_string());
    }
    if tool_bins.web.is_none() {
        app.push_note(rust_i18n::t!("errors.tool-web-not-found").to_string());
    }

    Ok((app, host_slot, project_root, tool_bins))
}

/// Build the host using the provider pool path, reading startup policy from
/// `~/.otto/config.toml`.
///
/// Each provider plugin's `try_build_registration` is wrapped in
/// `tokio::time::timeout(connect_timeout_ms)`. Timeout and build failures are
/// warned rather than failing startup — missing providers mean the user runs
/// `/connect` later.
///
/// Returns `(host, initial_model, initial_provider_id, deferred_notes)`.
/// `deferred_notes` collects timeout warnings that should be shown in the TUI
/// once `App` exists.
///
/// The legacy `OTTO_PROVIDER_URL` path is still supported: when that env
/// var is set we skip the pool path entirely and fall back to the rmcp HTTP
/// transport (the headless debug workflow).
///
async fn bootstrap_pool_host(
    project_root: &Path,
    tool_bins: &ToolBins,
    config_file: &config_file::ConfigFile,
    mcp_server_diagnostics: Vec<String>,
) -> HostBoot {
    // Legacy MCP-over-HTTP debug path. When this env var is set the host
    // connects to a remote provider binary instead of using the in-process
    // pool. No pool, no policy, no timeout wrapping.
    if let Ok(url) = std::env::var("OTTO_PROVIDER_URL") {
        let model = std::env::var("OTTO_MODEL").unwrap_or_else(|_| "claude-haiku-4-5".to_string());
        match start_host_remote(url, model.clone(), project_root.to_path_buf(), tool_bins).await {
            Ok(host) => {
                let notes = host.take_startup_notes();
                return HostBoot {
                    host: Some(host),
                    header_model: model,
                    provider_id: None,
                    startup_notes: notes,
                    mcp_manager_seed: McpManagerSeed::default(),
                    startup_verbose: config_file.startup.verbose,
                };
            }
            Err(e) => {
                eprintln!("warning: OTTO_PROVIDER_URL set but connect failed: {e:#}");
            }
        }
    }

    use crate::plugin::builtin::{
        provider_anthropic::ProviderAnthropicPlugin, provider_deepseek::ProviderDeepSeekPlugin,
        provider_gemini::ProviderGeminiPlugin, provider_grok::ProviderGrokPlugin,
        provider_local::ProviderLocalPlugin, provider_openai::ProviderOpenAiPlugin,
    };

    let timeout_dur = Duration::from_millis(config_file.startup.connect_timeout_ms);
    let mut providers: Vec<ProviderRegistration> = Vec::new();
    let mut deferred_notes: Vec<String> = mcp_server_diagnostics;

    // Try each provider plugin in priority order. Timeout and build errors
    // are non-fatal: the user can `/connect` any provider later.
    macro_rules! try_provider {
        ($plugin:expr, $log_name:literal, $spec_id:literal) => {{
            match tokio::time::timeout(timeout_dur, $plugin.try_build_registration()).await {
                Ok(Ok(crate::plugin::builtin::provider_common::ProviderBuildOutcome::Ready(reg, fallback_note))) => {
                    providers.push(reg);
                    if let Some(note) = fallback_note {
                        if config_file.startup.verbose {
                            deferred_notes.push(note);
                        }
                    }
                }
                Ok(Ok(crate::plugin::builtin::provider_common::ProviderBuildOutcome::Unavailable)) => {
                    // No credentials stored; user will /connect later.
                }
                Ok(Ok(crate::plugin::builtin::provider_common::ProviderBuildOutcome::Rejected {
                    reason,
                    kind: _,
                })) => {
                    // A key was found but list_models rejected it (bad key,
                    // no credit, rate-limited, org disabled, ...). Don't
                    // register a falsely-healthy provider; always log, only
                    // surface a transcript note when verbose.
                    tracing::warn!(plugin = $log_name, reason = %reason, "provider key rejected at startup");
                    if config_file.startup.verbose {
                        deferred_notes.push(rust_i18n::t!(
                            "notes.startup-build-failed",
                            name = $log_name,
                            err = reason,
                            id = $spec_id
                        ).to_string());
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!(plugin = $log_name, error = %e, "provider build failed at startup");
                    if config_file.startup.verbose {
                        deferred_notes.push(rust_i18n::t!(
                            "notes.startup-build-failed",
                            name = $log_name,
                            err = e.to_string(),
                            id = $spec_id
                        ).to_string());
                    }
                }
                Err(_elapsed) => {
                    let ms = config_file.startup.connect_timeout_ms;
                    tracing::warn!(
                        plugin = $log_name,
                        timeout_ms = ms,
                        "provider auto-connect timed out"
                    );
                    if config_file.startup.verbose {
                        deferred_notes.push(rust_i18n::t!(
                            "notes.startup-timeout",
                            name = $log_name,
                            ms = ms.to_string(),
                            id = $spec_id
                        ).to_string());
                    }
                }
            }
        }};
    }

    try_provider!(ProviderAnthropicPlugin::new(), "Anthropic", "anthropic");
    try_provider!(ProviderGeminiPlugin::new(), "Gemini", "gemini");
    try_provider!(ProviderOpenAiPlugin::new(), "OpenAI", "openai");
    try_provider!(ProviderDeepSeekPlugin::new(), "DeepSeek", "deepseek");
    try_provider!(ProviderGrokPlugin::new(), "xAI Grok", "grok");
    try_provider!(ProviderLocalPlugin::new(), "Local (Ollama)", "local");

    if providers.is_empty() {
        return HostBoot {
            host: None,
            header_model: "(disconnected)".to_string(),
            provider_id: None,
            startup_notes: deferred_notes,
            mcp_manager_seed: build_disconnected_mcp_manager_seed(config_file),
            startup_verbose: false,
        };
    }

    // Determine the initial active provider. `Host::start` will connect the
    // first provider that passes the startup_connect filter; mirror that logic
    // here to derive the header model + provider-id hint.
    let startup_policy = config_file.to_startup_policy();
    let active_reg = {
        use otto_host::StartupConnectPolicy;
        // Build a predicate that mirrors Host::start's filtering logic so
        // we can compute the initial model hint without waiting for the host.
        let allow_set: Option<std::collections::HashSet<_>> = match &startup_policy {
            StartupConnectPolicy::All => None,
            StartupConnectPolicy::None => Some(std::collections::HashSet::new()),
            StartupConnectPolicy::OptIn(allow) | StartupConnectPolicy::LastUsed(allow) => {
                Some(allow.iter().cloned().collect())
            }
            // Non-exhaustive: future variants default to no filter (same as All).
            _ => None,
        };
        providers
            .iter()
            .find(|r| match &allow_set {
                None => true, // All
                Some(set) => set.contains(&r.id),
            })
            .or_else(|| providers.first())
    };

    // Run the OTTO_MODEL legacy resolver against the full provider list.
    // This handles both "provider/model" and bare-model forms and surfaces
    // diagnostics when the value is ambiguous or unknown.
    let env_model_raw = std::env::var("OTTO_MODEL").unwrap_or_default();
    let provider_views: Vec<ProviderView<'_>> = providers
        .iter()
        .map(|r| ProviderView {
            id: &r.id,
            capabilities: &r.capabilities,
        })
        .collect();
    let legacy_resolution = resolve_legacy_model(&env_model_raw, &provider_views);

    // For Resolved/ResolvedFromBare the resolver picked a specific provider;
    // override active_reg to that registration so the header and HostConfig
    // both reflect the correct initial model. For all other outcomes the
    // existing policy-filtered active_reg stands.
    let (resolved_model, resolved_reg): (Option<String>, Option<&ProviderRegistration>) =
        match &legacy_resolution {
            LegacyModelResolution::Resolved { provider, model } => {
                let reg = providers.iter().find(|r| &r.id == provider);
                (Some(model.clone()), reg)
            }
            LegacyModelResolution::ResolvedFromBare {
                provider,
                model,
                note,
            } => {
                deferred_notes.push(note.clone());
                let reg = providers.iter().find(|r| &r.id == provider);
                (Some(model.clone()), reg)
            }
            LegacyModelResolution::Ambiguous { note, .. }
            | LegacyModelResolution::Unknown { note }
            | LegacyModelResolution::UnknownProvider { note, .. } => {
                deferred_notes.push(note.clone());
                (None, None)
            }
            LegacyModelResolution::NoOverride => (None, None),
            // Non-exhaustive: future variants fall through to no override.
            _ => (None, None),
        };

    let effective_reg = resolved_reg.or(active_reg);

    let (initial_model, initial_provider_id) = match effective_reg {
        Some(reg) => {
            // Use the resolved model when the legacy resolver found one;
            // otherwise fall back to the persisted models.toml preference or
            // the provider's capability default_model.
            let model = if let Some(m) = resolved_model {
                m
            } else {
                let base = reg.capabilities.default_model_id().to_string();
                let pref = models_pref::ModelsPref::load();
                if let Some(persisted) = pref.get(reg.id.as_str()) {
                    persisted.to_string()
                } else if let Some(d) = crate::routing_pref::load_default_pick() {
                    if d.provider == reg.id { d.model } else { base }
                } else {
                    base
                }
            };
            // Map the ProviderRegistration id back to a `&'static str` by
            // looking it up in the runtime provider catalog (built-ins +
            // any wasm providers installed at startup). This is only
            // used for the header display + active_provider_id.
            let static_id = effective_providers()
                .into_iter()
                .find(|s| s.id == reg.id.as_str())
                .map(|s| s.id);
            (model, static_id)
        }
        None => ("(disconnected)".to_string(), None),
    };

    let mut config = tool_bins.apply(
        HostConfig::new(
            // Legacy `provider` field is unused when `providers` is non-empty.
            // Pass a recognisable placeholder so any accidental log lines say
            // where they came from.
            ProviderEndpoint::StreamableHttp {
                url: "inproc://pool".into(),
            },
            initial_model.clone(),
        )
        .with_project_root(project_root.to_path_buf())
        .with_app_version(env!("CARGO_PKG_VERSION")),
    );
    config.providers = providers;
    config.startup_connect = startup_policy;
    config.connect_timeout_ms = config_file.startup.connect_timeout_ms;
    config.routing_rules_path = crate::routing_pref::routing_toml_path();
    let (mcp_tools, mcp_manager_seed) =
        resolve_configured_mcp_servers(config_file, &mut deferred_notes).await;
    for endpoint in mcp_tools {
        config.tools.push(endpoint);
    }

    match Host::start(config).await {
        Ok(host) => {
            // Surface any one-shot startup notes the host recorded (e.g.
            // a `routing.toml` parse failure). These are appended to
            // `deferred_notes` so `run_app` pushes them once `App` exists.
            deferred_notes.extend(host.take_startup_notes());
            let host_arc = Arc::new(host);
            host_arc.wire_self_arc();
            HostBoot {
                host: Some(host_arc),
                header_model: initial_model,
                provider_id: initial_provider_id,
                startup_notes: deferred_notes,
                mcp_manager_seed,
                startup_verbose: config_file.startup.verbose,
            }
        }
        Err(e) => {
            eprintln!("warning: pool host start failed: {e:#}");
            HostBoot {
                host: None,
                header_model: "(disconnected)".to_string(),
                provider_id: None,
                startup_notes: deferred_notes,
                mcp_manager_seed,
                startup_verbose: false,
            }
        }
    }
}

async fn resolve_configured_mcp_servers(
    config_file: &config_file::ConfigFile,
    deferred_notes: &mut Vec<String>,
) -> (Vec<ToolEndpoint>, McpManagerSeed) {
    let oauth_timeout = crate::mcp_oauth::startup_network_timeout(Duration::from_millis(
        config_file.startup.connect_timeout_ms,
    ));
    let mut configured = Vec::new();
    let mut skip_notes = Vec::new();
    let mut seen_names = std::collections::HashSet::new();
    let mut endpoints = Vec::new();

    for entry in &config_file.mcp_servers {
        let summary = summarize_mcp_server(entry);
        let name = summary.name.clone();
        configured.push(summary);

        if let Err(reason) = entry.validate(&seen_names) {
            deferred_notes.push(format!("mcp server `{name}` skipped: {reason}"));
            skip_notes.push((name, reason));
            continue;
        }
        seen_names.insert(name.clone());

        let endpoint = match entry {
            config_file::McpServerEntry::Stdio {
                name,
                command,
                args,
                env,
            } => match resolve_mcp_stdio_env(name, env) {
                Ok(env) => ToolEndpoint::Stdio {
                    name: name.clone(),
                    command: PathBuf::from(command),
                    args: args.clone(),
                    env,
                },
                Err(reason) => {
                    deferred_notes.push(format!("mcp server `{name}` skipped: {reason}"));
                    skip_notes.push((name.clone(), reason));
                    continue;
                }
            },
            config_file::McpServerEntry::Http { name, url, auth } => {
                match resolve_mcp_http_auth(name, url, auth, oauth_timeout).await {
                    Ok(auth) => ToolEndpoint::Http {
                        name: name.clone(),
                        url: url.clone(),
                        auth,
                    },
                    Err(reason) => {
                        deferred_notes.push(format!("mcp server `{name}` skipped: {reason}"));
                        skip_notes.push((name.clone(), reason));
                        continue;
                    }
                }
            }
        };
        endpoints.push(endpoint);
    }

    (
        endpoints,
        McpManagerSeed {
            configured,
            skip_notes,
        },
    )
}

fn resolve_mcp_stdio_env(
    server_name: &str,
    env: &std::collections::HashMap<String, String>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let mut resolved = std::collections::HashMap::new();
    for (key, value) in env {
        if value == "keyring" {
            let Some(secret) = creds::mcp_load(server_name)
                .map_err(|err| format!("failed to read keyring secret: {err}"))?
            else {
                return Err(format!("missing keyring secret for `mcp:{server_name}`"));
            };
            resolved.insert(key.clone(), secret);
        } else {
            resolved.insert(key.clone(), value.clone());
        }
    }
    Ok(resolved)
}

async fn resolve_mcp_http_auth(
    server_name: &str,
    url: &str,
    auth: &config_file::McpAuthMode,
    oauth_timeout: Duration,
) -> Result<HttpAuth, String> {
    match auth {
        config_file::McpAuthMode::None => Ok(HttpAuth::None),
        config_file::McpAuthMode::Bearer => {
            let Some(token) = creds::mcp_load(server_name)
                .map_err(|err| format!("failed to read keyring secret: {err}"))?
            else {
                return Err(format!("missing keyring secret for `mcp:{server_name}`"));
            };
            Ok(HttpAuth::Bearer { token })
        }
        config_file::McpAuthMode::Oauth => match crate::mcp_oauth::load_stored_secret(server_name)?
        {
            Some(stored) => {
                crate::mcp_oauth::build_startup_http_auth(server_name, url, stored, oauth_timeout)
                    .await
            }
            None => Err(format!(
                "missing oauth keyring state for `mcp:{server_name}`; open /mcp to authorize"
            )),
        },
    }
}

async fn start_host_remote(
    url: String,
    model: String,
    project_root: PathBuf,
    tool_bins: &ToolBins,
) -> Result<Arc<Host>> {
    let mut config = tool_bins.apply(
        HostConfig::new(ProviderEndpoint::StreamableHttp { url }, model)
            .with_project_root(project_root)
            .with_app_version(env!("CARGO_PKG_VERSION")),
    );
    config.routing_rules_path = crate::routing_pref::routing_toml_path();
    let host = Host::start(config).await.context("failed to start host")?;
    let host_arc = Arc::new(host);
    host_arc.wire_self_arc();
    Ok(host_arc)
}

/// Resolve a bundled tool-server binary by name. Tries (in order):
///
/// 1. `<env_override>` env var (must point at an existing file).
/// 2. A sibling of the running TUI executable — i.e. `target/<profile>/`
///    when launched via `cargo run`, or the install dir when installed.
/// 3. Bare `<name>` resolved via `PATH`.
///
/// Returns `None` if none of the candidates exists. The caller surfaces a
/// note so the user knows that tool surface is disabled.
fn locate_bundled_bin(name: &str, env_override: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var(env_override) {
        let path = PathBuf::from(p);
        return path.exists().then_some(path);
    }
    let bin_name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(&bin_name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(&bin_name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

pub(crate) async fn current_host(slot: &HostSlot) -> Option<Arc<Host>> {
    slot.read().await.clone()
}

fn init_tracing() {
    // The TUI owns the terminal once it enters ratatui's alternate screen,
    // so tracing must NEVER write to stderr — a single line would corrupt
    // the rendered UI. Route everything to a daily append log under
    // `~/.otto/logs/`. If the file can't be opened (no $HOME, perms),
    // fall back to a sink so we still register a global subscriber (and
    // therefore still silence stderr from `tracing` macros) instead of
    // landing back on the default stderr writer.
    let writer: Box<dyn std::io::Write + Send + 'static> = match open_otto_log() {
        Ok(file) => Box::new(file),
        Err(_) => Box::new(std::io::sink()),
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(writer))
        .try_init();
}

fn open_otto_log() -> std::io::Result<std::fs::File> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is unset"))?;
    let dir = PathBuf::from(home).join(".otto").join("logs");
    std::fs::create_dir_all(&dir)?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("otto.log"))
}

fn transcript_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".otto").join("transcripts")
}

pub(crate) async fn save_transcript_now(app: &App, host: &Arc<Host>) -> Result<PathBuf> {
    if app.entries.is_empty() {
        return Ok(PathBuf::new());
    }
    // Collect interactive-state snapshots from every live canvas renderer
    // and push them into the host's stored `Html` blocks before the
    // transcript is serialized. The renderer's `ContentBlockId.0` equals
    // the canvas's stream ordinal among top-level `Html` blocks, which is
    // exactly the index `set_canvas_states` walks.
    let states: Vec<(u32, String)> = app
        .canvas_registry
        .iter_renderers()
        .filter_map(|(id, r)| {
            r.snapshot_state().map(|bytes| {
                use base64::Engine as _;
                (
                    id.0,
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                )
            })
        })
        .collect();
    if !states.is_empty() {
        host.set_canvas_states(&states).await;
    }
    let path = app.transcript_path.read().await.clone();
    host.save_transcript(&path)
        .await
        .context("save transcript")?;
    Ok(path)
}

/// Run a slash command and apply its side effects. Used both when the user
/// types `/foo` + Enter and when they pick a no-arg command from the
/// palette. Commands that need direct host access (`/tools`, `/model`,
/// `/bash`, `/sandbox`, `/theme`, `/resume`) are dispatched here first.
/// All other slash commands are routed through the plugin SlashRouter; on
/// `SlashError::Unknown` we fall back to the legacy `App::handle_command`
/// for backwards compatibility with commands not yet ported to plugins.
pub(crate) async fn dispatch_slash_command(
    app: &mut App,
    cmd: &str,
    host_slot: &HostSlot,
    project_root: &Path,
    tool_bins: &ToolBins,
    worker_tx: &mpsc::Sender<WorkerMsg>,
) {
    let trimmed = cmd.trim_start();
    // `/bash` preserves the unparsed remainder verbatim (no `trim()` on
    // `rest`) so quoting like `--net   curl …` survives.
    let (head, rest_raw) = match trimmed.split_once(char::is_whitespace) {
        Some((h, r)) => (h, r),
        None => (trimmed, ""),
    };
    let rest = rest_raw.trim();

    match head {
        "/tools" => {
            show_tools(app, host_slot).await;
            return;
        }
        // Typed-arg invocations (`/model <id>`) and the no-host case still
        // flow through `handle_model_command`'s path: it validates the id
        // against the active provider's capability metadata, rebuilds the
        // host, and surfaces a useful note when nothing is connected yet.
        // With no args AND an active host, the guard is false, the arm
        // doesn't match, and control falls through to the plugin router so
        // the `internal:model` plugin can open the picker screen.
        "/model" if !rest.is_empty() || current_host(host_slot).await.is_none() => {
            handle_model_command(app, rest, host_slot).await;
            return;
        }
        "/resume" => {
            handle_resume_command(app, rest, host_slot).await;
            return;
        }
        "/sandbox" => {
            handle_sandbox_command(app, rest, host_slot).await;
            return;
        }
        "/bash" => {
            handle_bash_slash_command(app, rest_raw, host_slot, worker_tx).await;
            return;
        }
        "/disconnect" => {
            handle_disconnect_command(app, rest, host_slot, worker_tx).await;
            return;
        }
        "/use" => {
            handle_use_command(app, rest, host_slot).await;
            return;
        }
        _ => {}
    }

    // Try the plugin SlashRouter before falling through to the legacy handler.
    // Parse "/cmd arg1 arg2..." into (name, args).
    let name_str = head.strip_prefix('/').unwrap_or(head);
    let args: Vec<String> = rest.split_whitespace().map(String::from).collect();
    if let (Some(reg), Some(idx)) = (&app.plugin_registry, &app.plugin_indexes) {
        let reg = reg.clone();
        let idx = idx.clone();

        // Collect the suppress_prompt_segments list for this slash before
        // dispatching. Look up under a brief synchronous lock; guards are
        // dropped before any `.await` below, keeping async discipline.
        let suppress: Vec<String> = {
            let reg_guard = reg.read().await;
            let idx_guard = idx.read().await;
            let result = idx_guard
                .slash
                .get(name_str)
                .and_then(|pid| reg_guard.get(pid))
                .and_then(|handle| handle.try_lock().ok().map(|g| g.manifest()))
                .map(|m| {
                    m.contributions
                        .slash_commands
                        .into_iter()
                        .find(|s| s.name == name_str)
                        .map(|s| s.suppress_prompt_segments)
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            drop(reg_guard); // Explicitly drop to ensure no long-held locks
            drop(idx_guard); // Explicitly drop to ensure no long-held locks
            result
        };
        // If the slash spec suppresses any prompt segments, tell the host
        // before dispatching so the next turn (which some slashes trigger
        // immediately via Effect::RunTurn) omits those segments.
        if !suppress.is_empty() {
            if let Some(host) = current_host(host_slot).await {
                host.set_turn_suppression(suppress);
            }
        }

        let effs_result = {
            let router = crate::plugin::slash::SlashRouter::new(idx.clone(), reg.clone());
            router.dispatch(name_str, args).await
        };
        match effs_result {
            Ok(effs) => {
                if let Err(e) = crate::plugin::effects::apply_effects(app, effs).await {
                    tracing::warn!(error = %e, command = %name_str, "apply_effects after textarea slash dispatch failed");
                    app.push_styled_note(otto_plugin::StyledLine::plain(
                        rust_i18n::t!("notes.command-failed", err = format!("{e:#}")).to_string(),
                    ));
                }
                apply_pending_model_change(app, host_slot, project_root, tool_bins).await;
                apply_pending_pool_add(app, host_slot, project_root, tool_bins, false).await;
                apply_pending_gate(app, host_slot).await;
                apply_pending_in_process_tools(app, host_slot).await;
                apply_pending_routing_reload(app, host_slot).await;
                apply_pending_routing_show(app, host_slot).await;
                apply_pending_prompt_segments_reload(app, host_slot).await;
                return;
            }
            Err(crate::plugin::slash::SlashError::Unknown(_)) => {
                // Fall through to legacy handle_command below.
            }
            Err(e) => {
                tracing::warn!(error = %e, command = %name_str, "textarea slash dispatch failed");
                app.push_styled_note(otto_plugin::StyledLine::plain(
                    rust_i18n::t!("notes.command-failed", err = format!("{e:#}")).to_string(),
                ));
                return;
            }
        }
    }

    // TODO: remove legacy fallback once all slash commands are plugin-driven.
    app.handle_command(cmd);
}

/// Render `/tools` output: one note per registered tool, with the policy's
/// no-args verdict as a coarse hint.
async fn show_tools(app: &mut App, host_slot: &HostSlot) {
    let Some(host) = current_host(host_slot).await else {
        app.push_note(rust_i18n::t!("notes.not-connected-tools").to_string());
        return;
    };
    let defs = host.tool_defs().await;
    if defs.is_empty() {
        app.push_note(rust_i18n::t!("notes.no-tools-registered").to_string());
        return;
    }
    app.push_note(rust_i18n::t!("notes.tools-count", count = defs.len()).to_string());
    for def in &defs {
        let verdict = host.default_verdict_for(&def.name);
        let label = match verdict {
            otto_host::Verdict::Allow => "allow",
            otto_host::Verdict::Ask { .. } => "ask",
            otto_host::Verdict::Deny { .. } => "deny",
        };
        let desc = if def.description.is_empty() {
            String::new()
        } else {
            format!(" — {}", def.description)
        };
        app.push_note(
            rust_i18n::t!(
                "notes.tools-entry",
                policy = label,
                name = def.name.clone(),
                desc = desc
            )
            .to_string(),
        );
    }
}

/// Validate `requested` against the provider's advertised `models`. Returns
/// `Ok(())` when the id is in the list, `Err(known_ids)` otherwise.
///
/// Used only by unit tests — production validation now goes through
/// [`Host::active_capabilities`] instead of a live `list_models` RPC.
#[cfg(test)]
fn validate_model_id<'a>(
    requested: &str,
    models: &'a [otto_host::ModelInfo],
) -> Result<(), Vec<&'a str>> {
    if models.iter().any(|m| m.id == requested) {
        Ok(())
    } else {
        Err(models.iter().map(|m| m.id.as_str()).collect())
    }
}

/// Outcome of asking the provider whether `requested` is a known model id.
///
/// Used only by unit tests — production validation now goes through
/// [`Host::active_capabilities`] instead of a live `list_models` RPC.
#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
enum ModelChangeOutcome {
    /// Switch to the new model. When `warning` is `Some`, push it as a note
    /// first so the user understands the validation outcome.
    Proceed { warning: Option<String> },
    /// Refuse the change. `note` describes why, including the known model list
    /// when one is available.
    Reject { note: String },
}

/// Decide what to do with a `/model <id>` request given the result of asking
/// the host for `list_models`.
///
/// Pure (no IO, no [`App`] mutation) so it can be unit-tested without standing
/// up a [`Host`] or worker channel.
///
/// Used only by unit tests — production validation now goes through
/// [`Host::active_capabilities`] instead of a live `list_models` RPC.
#[cfg(test)]
fn resolve_model_change(
    requested: &str,
    list_result: Result<&otto_host::ListModelsResponse, &otto_protocol::ProviderError>,
) -> ModelChangeOutcome {
    match list_result {
        Ok(resp) if resp.models.is_empty() => {
            // The provider advertised the tool but returned no models. Rather
            // than reject every id against an empty "Known: " list, treat
            // this as "nothing to validate against" and proceed.
            ModelChangeOutcome::Proceed {
                warning: Some(
                    rust_i18n::t!("notes.model-no-models-optimistic", model = requested)
                        .to_string(),
                ),
            }
        }
        Ok(resp) => match validate_model_id(requested, &resp.models) {
            Ok(()) => ModelChangeOutcome::Proceed { warning: None },
            Err(known) => ModelChangeOutcome::Reject {
                note: rust_i18n::t!(
                    "notes.model-unknown-id",
                    model = requested,
                    known = known.join(", ")
                )
                .to_string(),
            },
        },
        Err(e) if matches!(e.kind, otto_protocol::ErrorKind::NotImplemented) => {
            // The provider doesn't advertise list_models at all. Silent
            // fall-through to the optimistic path is the contract.
            ModelChangeOutcome::Proceed { warning: None }
        }
        Err(e) => {
            // Network/auth/decode failure. Surface it to the user so they can
            // tell a typo'd id apart from a misconfigured key.
            ModelChangeOutcome::Proceed {
                warning: Some(
                    rust_i18n::t!(
                        "notes.model-verify-failed",
                        model = requested,
                        err = e.message.clone()
                    )
                    .to_string(),
                ),
            }
        }
    }
}

/// `/model` (no args) shows the current model. `/model <id>` validates the
/// requested id against the active provider's capability metadata and then
/// reconnects the active provider with the new id. When the pool has no
/// models registered for the active provider the check is skipped and the
/// provider rejects an invalid id at first turn instead.
///
/// Phase 3+: `/model <provider>/<model>` switches both the active provider
/// and the default model in one step — picking any row from the picker
/// (or typing the qualifier inline) routes future turns through that
/// provider.
async fn handle_model_command(app: &mut App, rest: &str, host_slot: &HostSlot) {
    if rest.is_empty() {
        match app.active_provider_id {
            Some(id) => app.push_note(
                rust_i18n::t!(
                    "notes.model-current-connected",
                    provider = id,
                    model = app.model.clone()
                )
                .to_string(),
            ),
            None => app.push_note(
                rust_i18n::t!(
                    "notes.model-current-not-connected",
                    model = app.model.clone()
                )
                .to_string(),
            ),
        }
        return;
    }

    // Parse the optional `provider/` qualifier off the front.
    let (qualifier, bare_model) = match rest.split_once('/') {
        Some((p, m)) if !p.is_empty() && !m.is_empty() => (Some(p.to_string()), m.to_string()),
        _ => (None, rest.to_string()),
    };

    // If a qualifier is present and differs from the current active
    // provider, swap providers first so the model validation lands on
    // the right pool entry.
    if let Some(prov) = &qualifier {
        if Some(prov.as_str()) != app.active_provider_id {
            let Some(host) = current_host(host_slot).await else {
                app.push_note(rust_i18n::t!("notes.model-not-connected").to_string());
                return;
            };
            let pid = match otto_protocol::ProviderId::new(prov) {
                Ok(p) => p,
                Err(_) => {
                    app.push_note(
                        rust_i18n::t!("notes.use-invalid-id", id = prov.clone()).to_string(),
                    );
                    return;
                }
            };
            match host.set_active_provider(&pid).await {
                Ok(()) => {
                    if let Some(s) = effective_providers()
                        .into_iter()
                        .find(|s| s.id == pid.as_str())
                    {
                        app.active_provider_id = Some(s.id);
                    }
                    if let Err(err) = crate::plugin::effects::dispatch_host_event(
                        app,
                        otto_plugin::HostEvent::ActiveProviderChanged { id: pid.clone() },
                        0,
                    )
                    .await
                    {
                        tracing::warn!(error = %err, "ActiveProviderChanged dispatch failed");
                    }
                }
                Err(otto_host::PoolError::NotRegistered(_)) => {
                    app.push_note(
                        rust_i18n::t!("notes.use-not-connected", name = prov.clone()).to_string(),
                    );
                    return;
                }
                Err(e) => {
                    app.push_note(format!("{e}"));
                    return;
                }
            }
        }
    }

    let Some(spec_id) = app.active_provider_id else {
        app.push_note(rust_i18n::t!("notes.model-not-connected").to_string());
        return;
    };
    if effective_providers().into_iter().all(|s| s.id != spec_id) {
        app.push_note(rust_i18n::t!("notes.model-unknown-provider", id = spec_id).to_string());
        return;
    }

    // Validate the requested id against the (now-)active provider's
    // capability metadata. When the pool has no capabilities registered
    // for the active provider (shouldn't happen in practice) we fall
    // through optimistically.
    if let Some(host) = current_host(host_slot).await {
        if let Some(caps) = host.active_capabilities().await {
            if !caps.models().is_empty() && caps.model(&bare_model).is_none() {
                let active = host.active_provider().await;
                app.push_note(
                    rust_i18n::t!(
                        "notes.model-not-in-active",
                        id = bare_model.clone(),
                        provider = active.as_str()
                    )
                    .to_string(),
                );
                return;
            }
        }
    }

    // Persist immediately for the direct /model <id> command path.
    if let Some(spec) = effective_providers().into_iter().find(|s| s.id == spec_id) {
        match models_pref::save_for_provider(spec.id, &bare_model).await {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!(error = ?e, provider = spec.id, "models.toml save failed");
                app.push_note(
                    rust_i18n::t!("notes.model-pref-save-failed", err = format!("{e:#}"))
                        .to_string(),
                );
            }
        }
    }

    perform_model_change(bare_model, host_slot, app).await;
    refresh_cached_models(app, host_slot).await;
}

/// `/resume` with no args opens the transcript picker. With a path arg,
/// loads the transcript immediately. Requires an active host connection;
/// if none exists, surfaces a clear error.
///
/// Refuses to run while a turn is in flight: `Host::run_turn_inner`
/// snapshots `state.messages` at turn start and commits its local clone
/// back at turn end, so a mid-turn `load_transcript` would be silently
/// overwritten when the in-flight turn finishes.
async fn handle_resume_command(app: &mut App, rest: &str, host_slot: &HostSlot) {
    if app.is_loading {
        app.push_note(rust_i18n::t!("notes.cannot-resume-during-turn").to_string());
        return;
    }
    if rest.is_empty() {
        // Open the picker — actual load happens when the user presses Enter
        // in `SelectingTranscript` mode (handled in `run_app`).
        app.open_transcript_picker(&app.transcript_dir.clone());
        return;
    }

    // Inline path argument: /resume <path>.
    let path = {
        let p = PathBuf::from(rest);
        if p.is_absolute() {
            p
        } else {
            // Treat bare names like "1715340000" or "1715340000.json" as
            // relative to the transcript directory.
            let candidate = app.transcript_dir.join(rest);
            if candidate.exists() {
                candidate
            } else {
                // Try adding .json extension.
                let with_ext = app.transcript_dir.join(format!("{rest}.json"));
                if with_ext.exists() { with_ext } else { p }
            }
        }
    };
    do_resume_from_path(app, host_slot, &path).await;
}

/// Load a transcript from `path` into the active host and replay it into
/// the conversation log.
async fn do_resume_from_path(app: &mut App, host_slot: &HostSlot, path: &Path) {
    let Some(host) = current_host(host_slot).await else {
        app.push_note(rust_i18n::t!("notes.cannot-resume-not-connected").to_string());
        return;
    };

    match host.load_transcript(path).await {
        Ok(record) => {
            // Warn if the transcript was from a different model.
            if record.model != host.config().model && record.saved_at > 0 {
                app.push_note(
                    rust_i18n::t!(
                        "notes.resume-model-mismatch",
                        saved = record.model.clone(),
                        current = host.config().model.clone()
                    )
                    .to_string(),
                );
            }
            let ts = if record.saved_at > 0 {
                collect_transcript_entries(path.parent().unwrap_or(path))
                    .into_iter()
                    .find(|e| e.path == path)
                    .map(|e| e.timestamp)
                    .unwrap_or_else(|| record.saved_at.to_string())
            } else {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .to_owned()
            };
            app.replay_transcript(&record);
            app.resumed_at = Some(ts.clone());
            app.push_note(
                rust_i18n::t!(
                    "notes.resume-resumed",
                    ts = ts,
                    count = record.messages.len()
                )
                .to_string(),
            );
        }
        Err(TranscriptError::SchemaMismatch { found, expected }) => {
            app.push_note(
                rust_i18n::t!(
                    "notes.resume-schema-mismatch",
                    found = found,
                    expected = expected
                )
                .to_string(),
            );
        }
        Err(TranscriptError::Malformed(msg)) => {
            app.push_note(rust_i18n::t!("notes.resume-malformed-json", msg = msg).to_string());
        }
        Err(TranscriptError::Io(e)) => {
            app.push_note(
                rust_i18n::t!("notes.resume-io-error", err = format!("{e:#}")).to_string(),
            );
        }
    }
}

/// Refresh [`App::cached_models`] from the *entire* connected pool's
/// capability metadata.
///
/// Phase 3+: the `/model` picker shows every connected provider's models,
/// not just the active provider's catalog — picking a row sets both the
/// default model and the default provider in one step. Each row's `id` is
/// qualified as `provider/model` so the picker (and downstream
/// `Effect::SetActiveModel` handler) can recover the destination provider
/// from the selection.
///
/// Falls back to a single-entry catalog containing the current model id
/// when no host is connected or the pool is empty, so the picker always
/// has something to render.
async fn refresh_cached_models(app: &mut App, host_slot: &HostSlot) {
    let fallback = vec![otto_plugin::ModelEntry {
        id: app.model.clone(),
        display_name: app.model.clone(),
    }];
    let Some(host) = current_host(host_slot).await else {
        app.cached_models = fallback;
        return;
    };
    let snapshot = host.pool_snapshot().await;
    if snapshot.is_empty() {
        tracing::debug!("pool_snapshot returned empty; using single-entry fallback");
        app.cached_models = fallback;
        return;
    }
    let mut rows: Vec<otto_plugin::ModelEntry> = Vec::new();
    for (pid, caps) in snapshot {
        for m in caps.models() {
            let display = if m.display_name.is_empty() {
                m.id.clone()
            } else {
                m.display_name.clone()
            };
            rows.push(otto_plugin::ModelEntry {
                // `provider/model` qualifier carries the destination
                // provider through the picker → SetActiveModel → host
                // round-trip. `apply_pending_model_change` parses the
                // prefix back out.
                id: format!("{}/{}", pid.as_str(), m.id),
                display_name: format!("{}/{} — {}", pid.as_str(), m.id, display),
            });
        }
    }
    if rows.is_empty() {
        tracing::debug!("pool_snapshot had no models across any provider; using fallback");
        app.cached_models = fallback;
        return;
    }
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    app.cached_models = rows;
}

/// Drain `app.pending_model_change` (set by `Effect::SetActiveModel`)
/// and forward the request to [`perform_model_change`]. No-op when
/// nothing is queued.
///
/// Phase 3+: the picker rows are qualified as `provider/model`. When a
/// pending id carries a `provider/` prefix, this function switches the
/// active provider first (so the model lands on the right host) and then
/// applies the bare model id. A bare id (no slash) is treated as a model
/// on the currently-active provider for backwards compatibility with the
/// `/model <id>` slash form.
pub(crate) async fn apply_pending_model_change(
    app: &mut App,
    host_slot: &HostSlot,
    _project_root: &Path,
    _tool_bins: &ToolBins,
) {
    let Some(pending) = app.pending_model_change.take() else {
        return;
    };

    // Parse the optional `provider/` qualifier off the front of the id.
    // When present, the target provider must be the one we set active
    // before applying the model change.
    let (target_provider, bare_model) = match pending.id.split_once('/') {
        Some((p, m)) if !p.is_empty() && !m.is_empty() => (Some(p.to_string()), m.to_string()),
        _ => (None, pending.id.clone()),
    };

    // If the picker handed us a `provider/model` id, switch the active
    // provider first. This must happen before `perform_model_change` so
    // `host.set_model` lands on the right pool entry.
    if let Some(prov) = &target_provider {
        let Some(host) = current_host(host_slot).await else {
            app.push_note(rust_i18n::t!("notes.model-not-connected").to_string());
            return;
        };
        let pid = match otto_protocol::ProviderId::new(prov) {
            Ok(p) => p,
            Err(_) => {
                app.push_note(rust_i18n::t!("notes.use-invalid-id", id = prov.clone()).to_string());
                return;
            }
        };
        if pid.as_str() != app.active_provider_id.unwrap_or("") {
            match host.set_active_provider(&pid).await {
                Ok(()) => {
                    if let Some(s) = effective_providers()
                        .into_iter()
                        .find(|s| s.id == pid.as_str())
                    {
                        app.active_provider_id = Some(s.id);
                    }
                    if let Err(err) = crate::plugin::effects::dispatch_host_event(
                        app,
                        otto_plugin::HostEvent::ActiveProviderChanged { id: pid.clone() },
                        0,
                    )
                    .await
                    {
                        tracing::warn!(error = %err, "ActiveProviderChanged dispatch failed");
                    }
                }
                Err(otto_host::PoolError::NotRegistered(_)) => {
                    app.push_note(
                        rust_i18n::t!("notes.use-not-connected", name = prov.clone()).to_string(),
                    );
                    return;
                }
                Err(e) => {
                    app.push_note(format!("{e}"));
                    return;
                }
            }
        }
    }

    let Some(spec_id) = app.active_provider_id else {
        app.push_note(rust_i18n::t!("notes.model-not-connected").to_string());
        return;
    };
    let Some(spec) = effective_providers().into_iter().find(|s| s.id == spec_id) else {
        app.push_note(rust_i18n::t!("notes.model-unknown-provider", id = spec_id).to_string());
        return;
    };

    // Validate the bare model id against the (now-)active provider's
    // capability metadata. When no capabilities are registered we proceed
    // optimistically — the provider rejects an invalid id at first turn
    // instead.
    if let Some(host) = current_host(host_slot).await {
        if let Some(caps) = host.active_capabilities().await {
            if !caps.models().is_empty() && caps.model(&bare_model).is_none() {
                let active = host.active_provider().await;
                app.push_note(
                    rust_i18n::t!(
                        "notes.model-not-in-active",
                        id = bare_model.clone(),
                        provider = active.as_str()
                    )
                    .to_string(),
                );
                return;
            }
        }
    }

    perform_model_change(bare_model.clone(), host_slot, app).await;

    // Refresh the picker's catalog cache; if the new model came with a
    // larger advertised set than the previous one, the picker should
    // see it on next open.
    refresh_cached_models(app, host_slot).await;

    // Persist only when the requesting effect asked for it.
    if pending.persist {
        match models_pref::save_for_provider(spec.id, &bare_model).await {
            Ok(()) => app
                .push_note(rust_i18n::t!("notes.model-persisted", provider = spec.id).to_string()),
            Err(e) => {
                tracing::warn!(error = ?e, provider = spec.id,
                    "models.toml save failed");
                app.push_note(
                    rust_i18n::t!("notes.model-pref-save-failed", err = format!("{e:#}"))
                        .to_string(),
                );
            }
        }
    }
}

/// Drain `app.pending_pool_add` (set by `Effect::RegisterProvider`) and
/// add the provider to the host's pool — building a fresh host first if
/// none exists yet. No-op when nothing is queued.
///
/// Rebuilds the `ProviderRegistration` via the matching plugin's
/// `try_build_registration` so the host pool gets a fresh
/// `Arc<dyn ProviderClient + Send + Sync>` — the silent-connect path's
/// boxed `dyn ProviderClient` can't be converted directly. The duplicate
/// client build is the price for fixing the silent-failure path where
/// `/connect <provider>` with a stored key only landed in
/// `App::registered_providers` and never the host pool. That rebuild also
/// validates the stored credential (`try_build_registration` calls
/// `list_models`), which is what lets this drain reject a bad key the
/// silent path itself couldn't check — the whole reason this function
/// exists rather than trusting the client the silent path already built.
///
/// Whether a host already exists must NOT gate this validation: previously,
/// finding `current_host` empty short-circuited the entire function before
/// the credential was even re-checked, so a rejected (or later re-entered
/// and valid) key silently vanished — no note, no host, and no record that
/// `/connect` had even been tried. That is exactly what stranded issue #81
/// on the DeepSeek path: it was reachable there whenever DeepSeek was
/// (re)connected as the very first provider of a session with no other
/// host already up, which — unlike `perform_connect`'s modal-submit path —
/// is precisely the case this drain exists to cover. Now the credential is
/// always re-validated and the outcome is always surfaced; a host is built
/// on demand (mirroring `perform_connect`'s first-connect branch) only when
/// validation actually succeeds.
///
/// `startup` distinguishes the one call site reached right after
/// `HostEvent::HostStarting`'s silent-connect subscribers run (`true`) from
/// the other call sites reached after the TUI event loop is already
/// running, in response to a slash command or bound action (`false`).
/// Notes are only unconditionally shown for the latter — the existing
/// `/connect`-adjacent UX. For the startup drain, notes are gated behind
/// `app.startup_verbose` so a revoked/rejected provider configured
/// alongside a healthy one doesn't reintroduce startup chatter.
pub(crate) async fn apply_pending_pool_add(
    app: &mut App,
    host_slot: &HostSlot,
    project_root: &Path,
    tool_bins: &ToolBins,
    startup: bool,
) {
    use crate::plugin::builtin::provider_common::ProviderBuildOutcome;
    use crate::plugin::builtin::{
        provider_anthropic::ProviderAnthropicPlugin, provider_deepseek::ProviderDeepSeekPlugin,
        provider_gemini::ProviderGeminiPlugin, provider_grok::ProviderGrokPlugin,
        provider_local::ProviderLocalPlugin, provider_openai::ProviderOpenAiPlugin,
    };

    let Some(pending) = app.pending_pool_add.take() else {
        return;
    };

    let spec = match effective_providers()
        .into_iter()
        .find(|s| s.id == pending.id.as_str())
    {
        Some(s) => s,
        None => {
            tracing::warn!(provider = %pending.id.as_str(),
                "Effect::RegisterProvider: id not in PROVIDERS catalog; cannot rebuild registration");
            return;
        }
    };

    let reg = match spec.id {
        "anthropic" => {
            ProviderAnthropicPlugin::new()
                .try_build_registration()
                .await
        }
        "gemini" => ProviderGeminiPlugin::new().try_build_registration().await,
        "openai" => ProviderOpenAiPlugin::new().try_build_registration().await,
        "deepseek" => ProviderDeepSeekPlugin::new().try_build_registration().await,
        "grok" => ProviderGrokPlugin::new().try_build_registration().await,
        "local" => ProviderLocalPlugin::new().try_build_registration().await,
        other => {
            tracing::warn!(
                provider = other,
                "apply_pending_pool_add: unknown provider id; cannot rebuild registration"
            );
            return;
        }
    };
    let show_notes = !startup || app.startup_verbose;
    let reg = match reg {
        Ok(ProviderBuildOutcome::Ready(r, fallback_note)) => {
            if let Some(note) = fallback_note {
                if show_notes {
                    app.push_note(note);
                }
            }
            r
        }
        Ok(ProviderBuildOutcome::Unavailable) => {
            // Key vanished from keyring between RegisterProvider being
            // emitted and this drainer running. Rare; surface a note so
            // the user knows what happened.
            tracing::warn!(provider = %pending.id.as_str(),
                "apply_pending_pool_add: credentials no longer available when rebuilding registration");
            if show_notes {
                app.push_note(
                    rust_i18n::t!("notes.connect-keyring-not-found", id = spec.id).to_string(),
                );
            }
            return;
        }
        Ok(ProviderBuildOutcome::Rejected { reason, kind }) => {
            // The stored key was rejected by list_models (bad key, no
            // credit, rate-limited, org disabled, ...). Don't register a
            // falsely-healthy provider.
            tracing::warn!(provider = %pending.id.as_str(), reason = %reason,
                "apply_pending_pool_add: provider key rejected");
            if show_notes {
                app.push_note(
                    rust_i18n::t!(
                        connect_rejected_note_key(kind, spec.api_key_required),
                        id = spec.id,
                        err = reason
                    )
                    .to_string(),
                );
            }
            return;
        }
        Err(e) => {
            tracing::warn!(provider = %pending.id.as_str(), error = %e,
                "apply_pending_pool_add: failed to rebuild registration");
            if show_notes {
                app.push_note(
                    rust_i18n::t!("notes.connect-failed", id = spec.id, err = format!("{e:#}"))
                        .to_string(),
                );
            }
            return;
        }
    };

    let registered_caps = reg.capabilities.clone();

    let host = match current_host(host_slot).await {
        Some(host) => {
            match host.add_provider(reg).await {
                Ok(()) => host,
                Err(otto_host::PoolError::AlreadyRegistered(_)) => {
                    // Already in the pool — the auto-connect at startup
                    // (bootstrap_pool_host) already added it. Nothing to do.
                    // Don't push a note; this is the common "double-emit" path
                    // where HostStarting auto-connect + bootstrap added the same
                    // provider via two routes.
                    tracing::debug!(provider = %pending.id.as_str(),
                        "apply_pending_pool_add: provider already in pool (likely bootstrap dup)");
                    return;
                }
                Err(e) => {
                    tracing::warn!(provider = %pending.id.as_str(), error = %e,
                        "apply_pending_pool_add: host.add_provider failed");
                    if show_notes {
                        app.push_note(
                            rust_i18n::t!(
                                "notes.connect-failed",
                                id = spec.id,
                                err = format!("{e}")
                            )
                            .to_string(),
                        );
                    }
                    return;
                }
            }
        }
        None => {
            // No host yet: the validated credential above is the first
            // provider of this session, so build a host for it instead of
            // dropping it on the floor (see the function doc for why this
            // used to be unreachable).
            if bootstrap_first_pool_host(
                spec,
                reg,
                &registered_caps,
                host_slot,
                project_root,
                tool_bins,
                app,
            )
            .await
            .is_err()
            {
                return;
            }
            match current_host(host_slot).await {
                Some(host) => host,
                None => {
                    tracing::error!(provider = %pending.id.as_str(),
                        "apply_pending_pool_add: bootstrap_first_pool_host reported success but left host_slot empty");
                    return;
                }
            }
        }
    };

    // Drift-repair + first-connect promotion, mirroring perform_connect's logic.
    let active_is_in_pool = host.active_capabilities().await.is_some();
    let should_promote = app.active_provider_id.is_none() || !active_is_in_pool;
    if should_promote {
        if !active_is_in_pool && app.active_provider_id.is_some() {
            tracing::warn!(
                old_active = ?app.active_provider_id,
                new_active = spec.id,
                "apply_pending_pool_add: host active_provider was not in pool; promoting just-added provider"
            );
        }
        app.active_provider_id = Some(spec.id);
        let (model, maybe_warning) =
            resolve_initial_model_for_with_caps(spec, Some(&registered_caps));
        if let Some(w) = maybe_warning {
            app.push_note(w);
        }
        app.model = model.clone();
        if let Ok(host_pid) = otto_protocol::ProviderId::new(spec.id) {
            if let Err(err) = host.set_active_provider(&host_pid).await {
                tracing::warn!(
                    error = %err,
                    new_active = spec.id,
                    "apply_pending_pool_add: host.set_active_provider failed"
                );
            }
        }
        host.set_model(model).await;
        app.refresh_splash_sandbox_from_host(host.sandbox_config());
        if let Ok(pid) = otto_plugin::ProviderId::new(spec.id) {
            if let Err(err) = crate::plugin::effects::dispatch_host_event(
                app,
                otto_plugin::HostEvent::ActiveProviderChanged { id: pid },
                0,
            )
            .await
            {
                tracing::warn!(error = %err,
                    "ActiveProviderChanged dispatch from apply_pending_pool_add failed");
            }
        }
    }
    refresh_cached_models(app, host_slot).await;
    app.push_note(rust_i18n::t!("notes.connected-to", name = pending.display_name).to_string());
}

/// Drain `app.pending_routing_reload` (set by `Effect::ReloadRoutingRules`)
/// and reload `~/.otto/routing.toml` via the host. No-op when nothing
/// is queued. Mirrors `apply_pending_model_change`'s drain pattern.
pub(crate) async fn apply_pending_routing_reload(app: &mut App, host_slot: &HostSlot) {
    if app.pending_routing_reload.take().is_none() {
        return;
    }
    let Some(host) = current_host(host_slot).await else {
        app.push_note(
            rust_i18n::t!("routing.reload-failed", err = "host not connected yet").to_string(),
        );
        return;
    };
    match host.reload_routing_rules().await {
        Ok(count) => {
            app.push_note(rust_i18n::t!("routing.reloaded", count = count.to_string()).to_string());
        }
        Err(e) => {
            app.push_note(rust_i18n::t!("routing.reload-failed", err = e.to_string()).to_string());
        }
    }
}

/// Drain `app.pending_routing_show` (set by `Effect::ShowRoutingRules`)
/// and render the routing-rules summary. No-op when nothing is queued.
pub(crate) async fn apply_pending_routing_show(app: &mut App, host_slot: &HostSlot) {
    if app.pending_routing_show.take().is_none() {
        return;
    }
    let Some(host) = current_host(host_slot).await else {
        app.push_note(rust_i18n::t!("routing.show-no-host").to_string());
        return;
    };
    let rules = host.routing_rules_snapshot().await;
    render_routing_show(app, &rules);
}

/// Render `/route show` output as plain styled notes onto `App`. Pure
/// function over the snapshot — no further host access required.
fn render_routing_show(app: &mut App, rules: &otto_host::RoutingRules) {
    if rules.rules.is_empty() {
        app.push_note(rust_i18n::t!("routing.show-no-rules").to_string());
    } else {
        app.push_note(rust_i18n::t!("routing.show-header").to_string());
        let connected: Vec<otto_protocol::ProviderId> = app.connected_provider_ids();
        for (i, rule) in rules.rules.iter().enumerate() {
            let idx = i + 1;
            let match_desc = format_rule_match(&rule.match_);
            let key = if connected.contains(&rule.use_.provider) {
                "routing.show-rule-line"
            } else {
                "routing.show-rule-skipped"
            };
            let line = rust_i18n::t!(
                key,
                index = idx.to_string(),
                name = rule.name.as_str(),
                r#match = match_desc,
                provider = rule.use_.provider.as_str(),
                model = rule.use_.model.as_str(),
            )
            .to_string();
            app.push_note(line);
        }
    }
    match &rules.default {
        Some(d) => app.push_note(
            rust_i18n::t!(
                "routing.show-default",
                provider = d.provider.as_str(),
                model = d.model.as_str()
            )
            .to_string(),
        ),
        None => app.push_note(rust_i18n::t!("routing.show-no-default").to_string()),
    }
    if rules.heuristics {
        app.push_note(rust_i18n::t!("routing.show-heuristics-active").to_string());
    }
    match app.most_recent_routing_decision() {
        Some((provider, model, reason)) => app.push_note(
            rust_i18n::t!(
                "routing.show-last",
                provider = provider,
                model = model,
                reason = reason
            )
            .to_string(),
        ),
        None => app.push_note(rust_i18n::t!("routing.show-no-last").to_string()),
    }
}

/// Drain `app.pending_prompt_segments_reload` (set by
/// `Effect::ReloadPromptSegments`) and re-push every enabled plugin's
/// current `SystemPromptSegment`s onto the active host, replacing what
/// it has. No-op when nothing is queued or when no host exists yet
/// (the plugin can re-emit the effect after `/connect` if needed).
/// Mirrors `apply_pending_routing_reload`'s guard-drop discipline: the
/// host-swap `Arc<RwLock<Option<Arc<Host>>>>`'s read guard is dropped by
/// `current_host` before we ever touch it, and the plugin registry's
/// `RwLock` guard is dropped (it's a temporary that ends with the
/// `active_prompt_segments()` call, a synchronous method) before the
/// synchronous `host.set_prompt_segments` call — no guard is held across
/// an `.await`.
pub(crate) async fn apply_pending_prompt_segments_reload(app: &mut App, host_slot: &HostSlot) {
    if app.pending_prompt_segments_reload.take().is_none() {
        return;
    }
    let Some(host) = current_host(host_slot).await else {
        tracing::warn!(
            "apply_pending_prompt_segments_reload: no host yet; reload dropped — \
             re-emit ReloadPromptSegments after /connect if needed"
        );
        return;
    };
    let Some(registry) = &app.plugin_registry else {
        tracing::warn!(
            "apply_pending_prompt_segments_reload: plugin runtime not installed; reload dropped"
        );
        return;
    };
    let segments = registry.read().await.active_prompt_segments();
    host.set_prompt_segments(segments);
}

/// Drain `app.pending_gate` (set by `Effect::RegisterPreToolGate`) and
/// install the gate on the active host. No-op when nothing is queued or
/// when no host exists yet (the effect arm already warns in that case via
/// a tracing::warn — the gate is simply dropped).
pub(crate) async fn apply_pending_gate(app: &mut App, host_slot: &HostSlot) {
    let Some(gate) = app.pending_gate.take() else {
        return;
    };
    let Some(host) = current_host(host_slot).await else {
        tracing::warn!(
            "apply_pending_gate: no host yet; gate dropped — \
             re-emit RegisterPreToolGate after /connect if needed"
        );
        return;
    };
    host.set_pre_tool_gate(gate).await;
}

/// Drain `app.pending_in_process_tools` (set by
/// `Effect::RegisterInProcessTool`) and register each pair with the
/// active host's `ToolRegistry`. No-op when the queue is empty or no
/// host exists yet. When the host exists but its registry slot is empty
/// (post-shutdown race), each tool is dropped with a warn-log; the
/// effect can be re-emitted by the plugin on the next host connection.
pub(crate) async fn apply_pending_in_process_tools(app: &mut App, host_slot: &HostSlot) {
    let queued = std::mem::take(&mut app.pending_in_process_tools);
    if queued.is_empty() {
        return;
    }
    let Some(host) = current_host(host_slot).await else {
        tracing::warn!(
            count = queued.len(),
            "apply_pending_in_process_tools: no host yet; tools dropped — \
             re-emit RegisterInProcessTool after /connect if needed"
        );
        return;
    };
    let Some(registry) = host.tool_registry_arc().await else {
        tracing::warn!(
            count = queued.len(),
            "apply_pending_in_process_tools: host registry slot empty; \
             tools dropped"
        );
        return;
    };
    for (spec, handler) in queued {
        let name = spec.name.clone();
        registry.register_in_process_tool(spec, handler).await;
        tracing::debug!(tool = %name, "registered in-process tool");
    }
}

fn format_rule_match(m: &otto_host::RuleMatch) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(b) = m.has_image {
        parts.push(format!("has_image={b}"));
    }
    if let Some(b) = m.has_pdf {
        parts.push(format!("has_pdf={b}"));
    }
    if let Some(b) = m.has_audio {
        parts.push(format!("has_audio={b}"));
    }
    if !m.keywords.is_empty() {
        parts.push(format!("keywords=[{}]", m.keywords.join(",")));
    }
    if let Some(n) = m.max_input_chars {
        parts.push(format!("max_input_chars={n}"));
    }
    if let Some(n) = m.min_input_chars {
        parts.push(format!("min_input_chars={n}"));
    }
    if parts.is_empty() {
        "<any>".to_string()
    } else {
        parts.join(", ")
    }
}

/// Resolve the effective model id for `provider_id`. Precedence (highest first):
///   OTTO_MODEL env > ~/.otto/models.toml > routing.toml#default > spec.default_model.
fn resolve_initial_model_for(spec: &ProviderSpec) -> String {
    resolve_initial_model_for_with_caps(spec, None).0
}

/// Same as [`resolve_initial_model_for`] but validates the
/// `routing.toml#default` step against the provider's actual capabilities
/// when `caps` is `Some`. Returns `(model, maybe_warning)` — the warning is
/// `Some` when `routing.toml#default` named a model that isn't in the
/// provider's catalog and the resolver fell back to `spec.default_model`.
/// The TUI surfaces the warning as a styled note so the user sees the
/// mismatch instead of a vendor `ModelNotFound` error on the first turn.
fn resolve_initial_model_for_with_caps(
    spec: &ProviderSpec,
    caps: Option<&otto_host::capabilities::ProviderCapabilities>,
) -> (String, Option<String>) {
    if let Ok(env_model) = std::env::var("OTTO_MODEL")
        && !env_model.is_empty()
    {
        // OTTO_MODEL diagnostics live in legacy_model::resolve_legacy_model
        // at bootstrap time; this path is the per-provider /connect resolution
        // and trusts whatever the user set.
        return (env_model, None);
    }
    let pref = models_pref::ModelsPref::load();
    if let Some(persisted) = pref.get(spec.id) {
        // /model only writes models from the active provider's catalog, so
        // this entry is trusted.
        return (persisted.to_string(), None);
    }
    if let Some(d) = crate::routing_pref::load_default_pick()
        && d.provider.as_str() == spec.id
    {
        // Validate against the provider's catalog when we have it. A typo
        // in routing.toml#default (e.g. "anthropic/from-routing-toml")
        // would otherwise surface as a vendor ModelNotFound on the first
        // turn; falling back here keeps the user moving and the warning
        // tells them where to look.
        if let Some(c) = caps {
            if c.model(&d.model).is_some() {
                return (d.model, None);
            }
            let warning = format!(
                "routing.toml default '{}/{}' references a model not in {}'s \
                 catalog; falling back to '{}'. Edit ~/.otto/routing.toml \
                 and run /route reload to silence this.",
                d.provider.as_str(),
                d.model,
                spec.id,
                spec.default_model,
            );
            return (spec.default_model.to_string(), Some(warning));
        }
        return (d.model, None);
    }
    (spec.default_model.to_string(), None)
}

/// Update the active model on the existing host without rebuilding it.
///
/// The pool is untouched — only the model field forwarded in future
/// `CompleteRequest`s changes. History and tool state are preserved.
/// Persistence to `~/.otto/models.toml` is the caller's responsibility
/// (both call sites — `handle_model_command` and `apply_pending_model_change`
/// — already handle that separately).
async fn perform_model_change(new_model: String, host_slot: &HostSlot, app: &mut App) {
    if let Some(host) = current_host(host_slot).await {
        host.set_model(new_model.clone()).await;
    }
    app.model = new_model;
    app.push_note(rust_i18n::t!("notes.model-is-now", model = app.model.clone()).to_string());
}

/// `/sandbox` (no args) shows current status.
/// `/sandbox on` / `/sandbox off` toggles the enabled flag and persists to
/// `~/.otto/sandbox.toml`.
///
/// Note: changing the setting takes effect the *next* time a host is built
/// (i.e. after `/connect`), because the sandbox is applied at tool spawn time
/// and tools are already running. The status display reflects the *current*
/// host's sandbox config (what was used when tools were spawned).
async fn handle_sandbox_command(app: &mut App, rest: &str, host_slot: &HostSlot) {
    match rest {
        "on" | "off" => {
            // Both subcommands set an *explicit* mode — the splash uses that
            // signal to suppress its v0.7-style nag banner.
            let new_mode = if rest == "on" {
                SandboxMode::On
            } else {
                SandboxMode::Off
            };
            let mut cfg = SandboxConfig::load();
            cfg.mode = new_mode;
            match cfg.save().await {
                Ok(()) => {
                    app.push_note(
                        rust_i18n::t!(
                            "notes.sandbox-enabled",
                            state = if new_mode == SandboxMode::On {
                                "enabled"
                            } else {
                                "disabled"
                            }
                        )
                        .to_string(),
                    );
                }
                Err(e) => {
                    app.push_note(
                        rust_i18n::t!("notes.sandbox-config-save-failed", err = format!("{e:#}"))
                            .to_string(),
                    );
                }
            }
        }
        "" => {
            // Show status from the currently-connected host.
            match current_host(host_slot).await {
                None => {
                    // No active host — show what's on disk instead.
                    let cfg = SandboxConfig::load();
                    app.push_note(
                        rust_i18n::t!(
                            "notes.sandbox-status-on-disk",
                            enabled = cfg.is_enabled().to_string(),
                            allow_net = cfg.allow_net.to_string(),
                            overrides = fmt_overrides(&cfg)
                        )
                        .to_string(),
                    );
                }
                Some(host) => {
                    let cfg = host.sandbox_config();
                    app.push_note(
                        rust_i18n::t!(
                            "notes.sandbox-status-active",
                            enabled = cfg.is_enabled().to_string(),
                            allow_net = cfg.allow_net.to_string(),
                            overrides = fmt_overrides(cfg)
                        )
                        .to_string(),
                    );
                    if cfg.is_enabled() {
                        #[cfg(target_os = "linux")]
                        app.push_note(rust_i18n::t!("notes.sandbox-wrapper-linux").to_string());
                        #[cfg(target_os = "macos")]
                        app.push_note(rust_i18n::t!("notes.sandbox-wrapper-macos").to_string());
                        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                        app.push_note(rust_i18n::t!("notes.sandbox-wrapper-windows").to_string());
                    }
                    if !cfg.extra_binds.is_empty() {
                        app.push_note(format!(
                            "  extra_binds: {}",
                            cfg.extra_binds
                                .iter()
                                .map(|p| p.display().to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                }
            }
        }
        other => {
            app.push_note(
                rust_i18n::t!("notes.sandbox-unknown-subcommand", sub = other).to_string(),
            );
        }
    }
}

/// `/disconnect <provider> [--force]` — remove a provider from the pool.
///
/// Without `--force`, uses [`DisconnectMode::Drain`]: new turns cannot
/// acquire this provider, but any in-flight turn is allowed to finish
/// naturally. With `--force`, uses [`DisconnectMode::Force`]: sends a
/// cooperative cancel and, if the grace period expires, hard-aborts every
/// registered in-flight task on that provider.
///
/// The actual `remove_provider` call is fire-and-forget via
/// `tokio::spawn` so the TUI input stays responsive during a long drain.
async fn handle_disconnect_command(
    app: &mut App,
    rest: &str,
    host_slot: &HostSlot,
    worker_tx: &mpsc::Sender<WorkerMsg>,
) {
    let mut tokens = rest.split_whitespace();
    let Some(provider) = tokens.next() else {
        app.push_note(rust_i18n::t!("notes.disconnect-needs-provider").to_string());
        return;
    };
    let force = tokens.any(|t| t == "--force");
    let Some(host) = current_host(host_slot).await else {
        app.push_note(rust_i18n::t!("notes.disconnect-no-host").to_string());
        return;
    };
    let pid = match otto_protocol::ProviderId::new(provider) {
        Ok(p) => p,
        Err(_) => {
            app.push_note(rust_i18n::t!("notes.disconnect-invalid-id", id = provider).to_string());
            return;
        }
    };
    if !host.is_connected(pid.as_str()).await {
        app.push_note(rust_i18n::t!("notes.disconnect-not-connected", name = provider).to_string());
        return;
    }
    let mode = if force {
        otto_host::DisconnectMode::Force
    } else {
        otto_host::DisconnectMode::Drain
    };
    let mode_label = if force { "force" } else { "drain" };
    app.push_note(
        rust_i18n::t!(
            "notes.disconnect-starting",
            name = provider,
            mode = mode_label
        )
        .to_string(),
    );
    let host_clone = std::sync::Arc::clone(&host);
    let provider_owned = provider.to_string();
    let mode_label_owned = mode_label.to_string();
    let tx = worker_tx.clone();
    tokio::spawn(async move {
        match host_clone.remove_provider(&pid, mode).await {
            Ok(()) => {
                let _ = tx
                    .send(WorkerMsg::DisconnectCompleted {
                        provider: provider_owned,
                        mode: mode_label_owned,
                    })
                    .await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "remove_provider failed in /disconnect handler");
                let _ = tx
                    .send(WorkerMsg::DisconnectFailed {
                        provider: provider_owned,
                        err: e.to_string(),
                    })
                    .await;
            }
        }
    });
}

async fn handle_use_command(app: &mut App, rest: &str, host_slot: &HostSlot) {
    let provider = rest.split_whitespace().next().unwrap_or("");
    if provider.is_empty() {
        app.push_note(rust_i18n::t!("notes.use-needs-provider").to_string());
        return;
    }
    let Some(host) = current_host(host_slot).await else {
        app.push_note(rust_i18n::t!("notes.use-no-host").to_string());
        return;
    };
    let pid = match otto_protocol::ProviderId::new(provider) {
        Ok(p) => p,
        Err(_) => {
            app.push_note(rust_i18n::t!("notes.use-invalid-id", id = provider).to_string());
            return;
        }
    };
    match host.set_active_provider(&pid).await {
        Ok(()) => {
            // Phase 3+: history is preserved across `/use`. The TUI
            // entries vector keeps every prior turn so the user can
            // continue the conversation on the new active provider.
            app.update_metrics();
            // Sync app.active_provider_id and app.model to the new active
            // provider so every subsequent site that branches on
            // app.active_provider_id sees the correct value.
            let spec = effective_providers().into_iter().find(|s| s.id == provider);
            if let Some(spec) = spec {
                app.active_provider_id = Some(spec.id);
                app.model = resolve_initial_model_for(spec);
                refresh_cached_models(app, host_slot).await;
            } else {
                // Shouldn't happen — set_active_provider already verified the
                // id exists in the pool — but defense-in-depth: log and skip
                // the model refresh rather than panicking.
                tracing::warn!(
                    provider = provider,
                    "handle_use_command: provider id not found in PROVIDERS; \
                     active_provider_id and model not updated"
                );
            }
            // Notify provider plugins so they can flip their active
            // marker in render_slot without polling.
            if let Err(err) = crate::plugin::effects::dispatch_host_event(
                app,
                otto_plugin::HostEvent::ActiveProviderChanged { id: pid.clone() },
                0,
            )
            .await
            {
                tracing::warn!(error = %err, "ActiveProviderChanged dispatch failed");
            }
            app.push_note(rust_i18n::t!("notes.use-switched", name = provider).to_string());
        }
        Err(otto_host::PoolError::NotRegistered(_)) => {
            app.push_note(rust_i18n::t!("notes.use-not-connected", name = provider).to_string());
        }
        Err(e) => {
            app.push_note(format!("{e}"));
        }
    }
}

/// `/bash <cmd>` — run `cmd` through `tool-bash` without round-tripping
/// through the provider. `--net` / `--no-net` flags at the front of `rest`
/// override the bash-network policy for this call only.
///
/// The call is dispatched on a worker task; its
/// [`TurnEvent`]s — most importantly any
/// [`TurnEvent::BashNetworkRequested`] the resolver emits — are forwarded
/// to the main loop's worker channel so the modal flow stays unchanged.
async fn handle_bash_slash_command(
    app: &mut App,
    rest_raw: &str,
    host_slot: &HostSlot,
    worker_tx: &mpsc::Sender<WorkerMsg>,
) {
    let parsed = match parse_bash_command(rest_raw) {
        Ok(p) => p,
        Err(BashCommandError::UnknownFlag { token }) => {
            app.push_note(rust_i18n::t!("notes.bash-flag-unknown", token = token).to_string());
            return;
        }
        Err(_) => {
            app.push_note(rust_i18n::t!("notes.bash-usage").to_string());
            return;
        }
    };
    let Some(host) = current_host(host_slot).await else {
        app.push_note(rust_i18n::t!("notes.not-connected-bash").to_string());
        return;
    };
    // Pre-flight: confirm tool-bash is actually configured. Without this
    // check the call falls through to `call_with_bash_net_override` and
    // surfaces as the opaque "unknown tool: run" error, which gives the
    // user no actionable repair path. The `run` tool is the contract
    // surface tool-bash advertises.
    let bash_configured = host.tool_defs().await.iter().any(|t| t.name == "run");
    if !bash_configured {
        app.push_note(rust_i18n::t!("notes.bash-not-configured").to_string());
        return;
    }
    if app.is_loading {
        app.push_note(rust_i18n::t!("notes.cannot-bash-during-turn").to_string());
        return;
    }

    // Surface the invocation in the transcript so its eventual result is
    // attached to a visible Tool entry (matches how model-driven calls
    // render).
    app.entries.push(Entry::Tool {
        name: "run".to_string(),
        args: serde_json::json!({ "command": parsed.command }),
        status: None,
        result_text: None,
    });
    app.is_loading = true;
    app.update_metrics();

    let tx = worker_tx.clone();
    let command = parsed.command.clone();
    let net_override = parsed.net_override;
    tokio::spawn(async move {
        let (ev_tx, mut ev_rx) = mpsc::channel::<TurnEvent>(8);
        let host_for_run = host.clone();
        let runner = tokio::spawn(async move {
            host_for_run
                .run_bash_command(&command, net_override, Some(ev_tx))
                .await
        });
        // Forward bash-network prompt events into the main loop. The
        // forwarder exits when `ev_tx` is dropped (i.e. the runner
        // returns).
        let forwarder_tx = tx.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(ev) = ev_rx.recv().await {
                if forwarder_tx.send(WorkerMsg::Event(ev)).await.is_err() {
                    break;
                }
            }
        });
        let outcome = runner.await;
        // Forwarder will drain on ev_tx drop; join it so we don't race a
        // dangling event past the final ToolCallFinished.
        let _ = forwarder.await;
        match outcome {
            Ok(Ok((is_error, payload))) => {
                let status = if is_error {
                    ToolCallStatus::Errored
                } else {
                    ToolCallStatus::Ok
                };
                let _ = tx
                    .send(WorkerMsg::Event(TurnEvent::ToolCallFinished {
                        name: "run".into(),
                        status,
                        result: payload,
                    }))
                    .await;
                let _ = tx.send(WorkerMsg::BashDone).await;
            }
            Ok(Err(msg)) => {
                let _ = tx.send(WorkerMsg::Error(format!("/bash: {msg}"))).await;
                let _ = tx.send(WorkerMsg::BashDone).await;
            }
            Err(join_err) => {
                let _ = tx
                    .send(WorkerMsg::Error(format!("/bash worker failed: {join_err}")))
                    .await;
                let _ = tx.send(WorkerMsg::BashDone).await;
            }
        }
    });
}

fn fmt_overrides(cfg: &SandboxConfig) -> String {
    if cfg.tool_overrides.is_empty() {
        return "(none)".into();
    }
    cfg.tool_overrides
        .iter()
        .map(|(k, v)| {
            let net = match v.allow_net {
                Some(true) => "net=yes",
                Some(false) => "net=no",
                None => "net=inherit",
            };
            format!("{k}({net})")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Build a fresh single-provider pool `Host` from an already-validated `reg`
/// and install it into `host_slot`. Shared by `perform_connect`'s
/// first-connect branch (a key just entered through the API-key modal) and
/// `apply_pending_pool_add`'s silent-reconnect branch (a stored key
/// re-validated after `Effect::RegisterProvider` fired with no host yet to
/// add it to) — both need identical "there is no host at all yet" bootstrap
/// logic, not just an additive `host.add_provider`.
///
/// Pushes its own failure note and returns `Err(())` on `Host::start`
/// failure so the caller can bail out the same way it does for every other
/// connect failure.
async fn bootstrap_first_pool_host(
    spec: &'static ProviderSpec,
    reg: ProviderRegistration,
    registered_caps: &otto_host::capabilities::ProviderCapabilities,
    host_slot: &HostSlot,
    project_root: &Path,
    tool_bins: &ToolBins,
    app: &mut App,
) -> Result<(), ()> {
    let (initial_model, maybe_warning) =
        resolve_initial_model_for_with_caps(spec, Some(registered_caps));
    if let Some(w) = maybe_warning {
        app.push_note(w);
    }
    let mut cfg = tool_bins.apply(
        HostConfig::new(
            ProviderEndpoint::StreamableHttp {
                url: "inproc://pool".into(),
            },
            initial_model,
        )
        .with_project_root(project_root.to_path_buf())
        .with_app_version(env!("CARGO_PKG_VERSION")),
    );
    cfg.providers = vec![reg];
    cfg.startup_connect = otto_host::StartupConnectPolicy::All;
    cfg.routing_rules_path = crate::routing_pref::routing_toml_path();
    match Host::start(cfg).await {
        Ok(h) => {
            // Surface any one-shot startup notes (e.g. routing.toml parse
            // failure) before the host is stashed in host_slot.
            for note in h.take_startup_notes() {
                app.push_note(note);
            }
            let host_arc = Arc::new(h);
            // Register the `Arc<Host>` back into the host so
            // `run_turn_inner` can construct a `ToolCallContext` for
            // in-process tools (the `task` tool from the user-agents
            // plugin, future built-ins, etc.).
            host_arc.wire_self_arc();
            *host_slot.write().await = Some(host_arc);
            Ok(())
        }
        Err(e) => {
            app.push_note(
                rust_i18n::t!("notes.connect-failed", id = spec.id, err = format!("{e:#}"))
                    .to_string(),
            );
            Err(())
        }
    }
}

/// Persist the key (if required), build the in-process handler, swap the host.
async fn perform_connect(
    spec: &'static ProviderSpec,
    api_key: String,
    host_slot: &HostSlot,
    project_root: &Path,
    tool_bins: &ToolBins,
    app: &mut App,
) {
    use crate::plugin::builtin::{
        provider_anthropic::ProviderAnthropicPlugin, provider_deepseek::ProviderDeepSeekPlugin,
        provider_gemini::ProviderGeminiPlugin, provider_grok::ProviderGrokPlugin,
        provider_local::ProviderLocalPlugin, provider_openai::ProviderOpenAiPlugin,
    };

    // 1. Persist the key so the plugin can read it back via keyring.
    if spec.api_key_required {
        if let Err(e) = creds::save(spec.id, &api_key) {
            app.push_note(
                rust_i18n::t!("notes.keyring-store-failed", err = format!("{e:#}")).to_string(),
            );
            return;
        }
    }

    app.push_note(rust_i18n::t!("notes.connecting-to", name = spec.display_name).to_string());

    // 2. Build the ProviderRegistration via the matching plugin. The plugin
    //    reads the key from the keyring; we just saved it above so the read
    //    will succeed. Surface any build failure with a note.
    let reg_result = match spec.id {
        "anthropic" => {
            ProviderAnthropicPlugin::new()
                .try_build_registration()
                .await
        }
        "gemini" => ProviderGeminiPlugin::new().try_build_registration().await,
        "openai" => ProviderOpenAiPlugin::new().try_build_registration().await,
        "deepseek" => ProviderDeepSeekPlugin::new().try_build_registration().await,
        "grok" => ProviderGrokPlugin::new().try_build_registration().await,
        "local" => ProviderLocalPlugin::new().try_build_registration().await,
        other => {
            app.push_note(rust_i18n::t!("notes.connect-unknown-provider", id = other).to_string());
            return;
        }
    };
    let reg = match reg_result {
        Ok(crate::plugin::builtin::provider_common::ProviderBuildOutcome::Ready(
            r,
            fallback_note,
        )) => {
            if let Some(note) = fallback_note {
                app.push_note(note);
            }
            r
        }
        Ok(crate::plugin::builtin::provider_common::ProviderBuildOutcome::Unavailable) => {
            // Keyring read returned nothing despite the just-saved key —
            // likely a backend issue.
            app.push_note(
                rust_i18n::t!("notes.connect-keyring-not-found", id = spec.id).to_string(),
            );
            return;
        }
        Ok(crate::plugin::builtin::provider_common::ProviderBuildOutcome::Rejected {
            reason,
            kind,
        }) => {
            // The just-saved key was rejected by list_models (bad key, no
            // credit, rate-limited, org disabled, ...). Surface the real
            // reason instead of the misleading "key not found" message —
            // this is a direct response to a user-initiated /connect, so
            // always shown (not gated by startup.verbose).
            app.push_note(
                rust_i18n::t!(
                    connect_rejected_note_key(kind, spec.api_key_required),
                    id = spec.id,
                    err = reason
                )
                .to_string(),
            );
            return;
        }
        Err(e) => {
            app.push_note(
                rust_i18n::t!("notes.connect-failed", id = spec.id, err = format!("{e:#}"))
                    .to_string(),
            );
            return;
        }
    };

    // Capture capabilities before `reg` is moved into the host so the
    // model-resolution step (step 4) can validate the resolved model
    // against this provider's catalog.
    let registered_caps = reg.capabilities.clone();

    // 3. Add to the pool, or build a first host when no host exists yet.
    //    The pool is additive — no history clear, no host replacement.
    let is_first_connect = current_host(host_slot).await.is_none();
    if is_first_connect {
        // No host yet — startup produced no registrations (e.g. user
        // dismissed the migration picker with startup_providers = []).
        // Build a fresh single-entry pool host.
        if bootstrap_first_pool_host(
            spec,
            reg,
            &registered_caps,
            host_slot,
            project_root,
            tool_bins,
            app,
        )
        .await
        .is_err()
        {
            return;
        }
    } else {
        // Pool already exists — replace the entry if this provider is already
        // connected (re-key), or add it fresh otherwise. `replace_provider`
        // handles both: see savvagent/otto#179.
        let host = current_host(host_slot).await.expect("checked above");
        if let Err(e) = host.replace_provider(reg).await {
            app.push_note(
                rust_i18n::t!("notes.connect-failed", id = spec.id, err = format!("{e}"))
                    .to_string(),
            );
            return;
        }
    }

    // 4. Update TUI state. Pool is additive — do NOT clear app.entries,
    //    live_text, or call clear_history. The conversation continues on the
    //    existing active provider.
    //
    // Drift detection: if the host's `active_provider` no longer resolves
    // to a pool entry (`active_capabilities()` is None — pre-Fix A this
    // could happen at startup when the policy filter emptied the pool but
    // left active pointing at a filtered-out id), promote the just-connected
    // provider to active so the next turn doesn't hit `NoActiveProvider`.
    // The condition is also a defense-in-depth catch for any future code
    // path that might drop the active provider out of the pool without
    // updating `active_provider`.
    let active_is_in_pool = match current_host(host_slot).await {
        Some(host) => host.active_capabilities().await.is_some(),
        None => false,
    };
    app.connected = true;
    let should_promote = app.active_provider_id.is_none() || !active_is_in_pool;
    if should_promote {
        if !active_is_in_pool && app.active_provider_id.is_some() {
            tracing::warn!(
                old_active = ?app.active_provider_id,
                new_active = spec.id,
                "perform_connect: host active_provider was not in pool; promoting just-connected provider"
            );
        }
        app.active_provider_id = Some(spec.id);
        let (model, maybe_warning) =
            resolve_initial_model_for_with_caps(spec, Some(&registered_caps));
        if let Some(w) = maybe_warning {
            app.push_note(w);
        }
        app.model = model.clone();
        // Note: cached_models is refreshed unconditionally below (after
        // the should_promote block) so every successful /connect refreshes
        // the /model picker.
        // Align the splash sandbox indicator with the newly-started host.
        if let Some(host) = current_host(host_slot).await {
            app.refresh_splash_sandbox_from_host(host.sandbox_config());
            // Update the host's active_provider AND current_model. The
            // is_first_connect=true branch built a fresh host with the
            // resolved model already set, so set_active_provider /
            // set_model are no-ops there; the drift-repair branch is
            // where both matter (the old host's current_model may still
            // be the stale value from startup).
            if let Ok(host_pid) = otto_protocol::ProviderId::new(spec.id) {
                if let Err(err) = host.set_active_provider(&host_pid).await {
                    tracing::warn!(
                        error = %err,
                        new_active = spec.id,
                        "perform_connect: host.set_active_provider failed"
                    );
                }
            }
            host.set_model(model).await;
        }
        // Tell provider plugins to flip their active marker.
        if let Ok(pid) = otto_plugin::ProviderId::new(spec.id) {
            if let Err(err) = crate::plugin::effects::dispatch_host_event(
                app,
                otto_plugin::HostEvent::ActiveProviderChanged { id: pid },
                0,
            )
            .await
            {
                tracing::warn!(error = %err, "ActiveProviderChanged dispatch from perform_connect failed");
            }
        }
    }
    // Refresh the `/model` picker on every successful connect (not just
    // promotion). Pre-fix this was nested inside the should_promote branch,
    // so the second `/connect` (where the active provider stays put) added
    // a provider to the pool whose models never made it into the picker —
    // `/model` looked stale or empty depending on what was cached before.
    refresh_cached_models(app, host_slot).await;
    app.push_note(rust_i18n::t!("notes.connected-to", name = spec.display_name).to_string());

    // 5. Dispatch ProviderRegistered + Connect for plugin subscribers.
    //
    // The `/connect <provider>` slash path emits `ProviderRegistered` +
    // `Connect` via `Effect::RegisterProvider`, but this legacy in-TUI
    // provider-picker flow registers directly — so without these dispatches
    // the splash HUD never flips to "Connected" and anything else subscribed
    // to `Connect` (telemetry, status indicators) is silently skipped.
    // Errors are warn-only so a buggy subscriber can't tank the connect.
    match otto_plugin::ProviderId::new(spec.id) {
        Ok(provider_id) => {
            if let Err(err) = crate::plugin::effects::dispatch_host_event(
                app,
                otto_plugin::HostEvent::ProviderRegistered {
                    id: provider_id.clone(),
                    display_name: spec.display_name.to_string(),
                },
                0,
            )
            .await
            {
                tracing::warn!(error = %err,
                    "ProviderRegistered dispatch from perform_connect failed");
            }
            if let Err(err) = crate::plugin::effects::dispatch_host_event(
                app,
                otto_plugin::HostEvent::Connect { provider_id },
                0,
            )
            .await
            {
                tracing::warn!(error = %err,
                    "Connect dispatch from perform_connect failed");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, provider_id = spec.id,
                "perform_connect: invalid provider id; skipping HostEvent dispatch");
        }
    }
}

/// Translate a streaming [`TurnEvent`] from the host into the corresponding
/// [`otto_plugin::HostEvent`], if any. Several `TurnEvent` variants
/// have no host-event analog (`IterationStarted` after the first,
/// `TextDelta`, `PermissionRequested`, `BashNetworkRequested`,
/// `ToolCallDenied`) and return `None`.
///
/// Mutates the four event-loop counters to keep turn/tool-call ids
/// monotonic and matched across Start/End pairs. The strict-sequential
/// nature of `Host::run_turn_inner` (tool calls don't interleave per
/// turn) makes a single `last_tool_call_id` slot sufficient.
pub(crate) fn translate_turn_event_to_host_event(
    event: &TurnEvent,
    next_turn_id: &mut u32,
    current_turn_id: &mut Option<u32>,
    next_tool_call_id: &mut u64,
    last_tool_call_id: &mut Option<u64>,
) -> Option<otto_plugin::HostEvent> {
    match event {
        TurnEvent::IterationStarted { iteration } => {
            if *iteration == 1 && current_turn_id.is_none() {
                *next_turn_id = next_turn_id.saturating_add(1);
                *current_turn_id = Some(*next_turn_id);
                Some(otto_plugin::HostEvent::TurnStart {
                    turn_id: *next_turn_id,
                })
            } else {
                None
            }
        }
        TurnEvent::ToolCallStarted { name, .. } => {
            *next_tool_call_id = next_tool_call_id.saturating_add(1);
            *last_tool_call_id = Some(*next_tool_call_id);
            Some(otto_plugin::HostEvent::ToolCallStart {
                call_id: next_tool_call_id.to_string(),
                tool: name.clone(),
            })
        }
        TurnEvent::ToolCallFinished { status, .. } => {
            // If we never saw a matching `ToolCallStarted`, this is a
            // synthetic `ToolCallFinished` synthesized by the `/bash`
            // direct-invocation path (`handle_bash_slash_command` skips
            // emitting `ToolCallStarted` because the host doesn't go
            // through `run_turn_streaming` there). Don't fabricate a
            // `call_id: "0"` orphan HostEvent — warn-log and skip
            // emission entirely.
            let Some(call_id) = last_tool_call_id.take() else {
                tracing::warn!(
                    "ToolCallFinished with no matching ToolCallStarted — skipping HostEvent \
                     emission (likely a /bash direct invocation)"
                );
                return None;
            };
            Some(otto_plugin::HostEvent::ToolCallEnd {
                call_id: call_id.to_string(),
                success: matches!(status, ToolCallStatus::Ok),
            })
        }
        TurnEvent::TurnComplete { .. } => {
            // Clear stale per-turn tool-call state so a future
            // `ToolCallFinished` without a matching `ToolCallStarted`
            // (e.g. via `/bash`) can't pick up a leaked id from the
            // previous turn.
            *last_tool_call_id = None;
            let turn_id = current_turn_id.take().unwrap_or(0);
            Some(otto_plugin::HostEvent::TurnEnd {
                turn_id,
                success: true,
            })
        }
        TurnEvent::Cancelled { .. } | TurnEvent::AbortedAfterGrace { .. } => {
            *last_tool_call_id = None;
            current_turn_id
                .take()
                .map(|turn_id| otto_plugin::HostEvent::TurnEnd {
                    turn_id,
                    success: false,
                })
        }
        TurnEvent::SubagentStop {
            agent_name,
            success,
        } => Some(otto_plugin::HostEvent::SubagentStop {
            agent_name: agent_name.clone(),
            success: *success,
        }),
        // No analog — these stay TUI-private.
        TurnEvent::RouteSelected { .. }
        | TurnEvent::ModalityWarning { .. }
        | TurnEvent::TextDelta { .. }
        | TurnEvent::PermissionRequested { .. }
        | TurnEvent::BashNetworkRequested { .. }
        | TurnEvent::ToolCallDenied { .. }
        | TurnEvent::ResourceUpdated { .. }
        | TurnEvent::HtmlBlockStart { .. }
        | TurnEvent::HtmlBlockDelta { .. }
        | TurnEvent::HtmlBlockStop { .. } => None,
    }
}

async fn dispatch_failed_turn_end_on_exit(
    app: &mut app::App,
    current_turn_id: &mut Option<u32>,
    footer_pending_turn_id: &mut Option<u32>,
    log_context: &'static str,
) {
    let (turn_id, synthesize_start) = if let Some(turn_id) = current_turn_id.take() {
        (turn_id, false)
    } else if let Some(turn_id) = footer_pending_turn_id.take() {
        (turn_id, true)
    } else {
        return;
    };
    if synthesize_start {
        if let Err(err) = crate::plugin::effects::dispatch_host_event(
            app,
            otto_plugin::HostEvent::TurnStart { turn_id },
            0,
        )
        .await
        {
            tracing::warn!(error = %err, context = log_context, "TurnStart(exit) dispatch failed");
        }
    }
    if let Err(err) = crate::plugin::effects::dispatch_host_event(
        app,
        otto_plugin::HostEvent::TurnEnd {
            turn_id,
            success: false,
        },
        0,
    )
    .await
    {
        tracing::warn!(error = %err, context = log_context, "TurnEnd(exit) dispatch failed");
    }
}

async fn record_turn_error(
    app: &mut App,
    message: String,
    footer_pending_turn_id: &mut Option<u32>,
    current_turn_id: &mut Option<u32>,
    next_turn_id: &mut u32,
    last_tool_call_id: &mut Option<u64>,
    turn_terminal_event_seen: &mut bool,
) {
    app.is_loading = false;
    let pending_turn_id = footer_pending_turn_id.take();
    app.entries.push(Entry::Note(format!("Error: {message}")));
    app.update_metrics();
    if !*turn_terminal_event_seen {
        // A runner error terminates the turn without a
        // terminal TurnEvent; emit TurnEnd { success: false }
        // so subscribers see symmetry with successful turns.
        // If the provider errored before producing
        // `IterationStarted { iteration: 1 }` (auth fail,
        // network glitch on first request), `current_turn_id`
        // is None — synthesize a TurnStart first so
        // subscribers see a complete `PromptSubmitted ->
        // TurnStart -> TurnEnd` shape instead of a missing
        // turn frame for those error modes.
        let turn_id = match current_turn_id.take() {
            Some(id) => id,
            None => {
                let synthetic = pending_turn_id.unwrap_or_else(|| {
                    *next_turn_id = next_turn_id.saturating_add(1);
                    *next_turn_id
                });
                *next_turn_id = (*next_turn_id).max(synthetic);
                if let Err(err) = crate::plugin::effects::dispatch_host_event(
                    app,
                    otto_plugin::HostEvent::TurnStart { turn_id: synthetic },
                    0,
                )
                .await
                {
                    tracing::warn!(error = %err, "synthetic TurnStart dispatch failed");
                }
                synthetic
            }
        };
        // Clear any stale per-turn tool-call state so the
        // next turn starts clean.
        *last_tool_call_id = None;
        if let Err(err) = crate::plugin::effects::dispatch_host_event(
            app,
            otto_plugin::HostEvent::TurnEnd {
                turn_id,
                success: false,
            },
            0,
        )
        .await
        {
            tracing::warn!(error = %err, "TurnEnd(failure) dispatch failed");
        }
    }
    *turn_terminal_event_seen = false;
}

/// Attempt to create a [`otto_plugin::ContentRenderer`] for the
/// `Entry::Canvas` identified by `canvas_id` and register it in
/// `app.canvas_registry`.
///
/// Called from the `run_app` event loop immediately after a
/// `TurnEvent::HtmlBlockStop` is processed (the sync half — moving
/// `source_preview` into `source` — already happened inside
/// `App::handle_html_block_stop`). This is the async half because it
/// needs to take read locks on the plugin indexes and registry.
///
/// Failures are warn-logged rather than propagated; the canvas entry
/// stays visible in the conversation log with its `source` field
/// populated even when no renderer is available (e.g. when the plugin
/// is disabled or the index hasn't been built yet).
pub(crate) async fn create_canvas_renderer(app: &mut App, canvas_id: otto_plugin::ContentBlockId) {
    // Extract the finalized source from the entry.
    let source = match app
        .entries
        .iter()
        .find(|e| matches!(e, Entry::Canvas { id, .. } if *id == canvas_id))
    {
        Some(Entry::Canvas { source, .. }) => source.clone(),
        _ => {
            tracing::debug!(?canvas_id, "create_canvas_renderer: canvas entry not found");
            return;
        }
    };

    // Look up the plugin that owns the canonical html renderer.
    let plugin_id = {
        let Some(indexes_arc) = app.plugin_indexes.as_ref() else {
            tracing::debug!("create_canvas_renderer: plugin_indexes not installed");
            return;
        };
        let indexes = indexes_arc.read().await;
        match indexes.content_renderer_for("html") {
            Some(id) => id.clone(),
            None => {
                tracing::debug!("create_canvas_renderer: no html renderer registered");
                return;
            }
        }
        // indexes guard drops here
    };

    // Retrieve the plugin and call create_renderer (sync).
    let renderer_result = {
        let Some(registry_arc) = app.plugin_registry.as_ref() else {
            tracing::debug!("create_canvas_renderer: plugin_registry not installed");
            return;
        };
        let registry = registry_arc.read().await;
        let Some(handle) = registry.get(&plugin_id) else {
            tracing::warn!(
                plugin_id = %plugin_id.as_str(),
                "create_canvas_renderer: plugin not found in registry"
            );
            return;
        };
        // Lock the plugin and call create_renderer (sync, no await inside).
        let guard = match handle.try_lock() {
            Ok(g) => g,
            Err(_) => {
                tracing::warn!(
                    plugin_id = %plugin_id.as_str(),
                    "create_canvas_renderer: plugin mutex contended"
                );
                return;
            }
        };
        guard.create_renderer("html", canvas_id, &source)
        // guard + registry drop here
    };

    match renderer_result {
        Ok(renderer) => {
            app.canvas_registry.insert(canvas_id, renderer);
            tracing::debug!(?canvas_id, "canvas renderer created and registered");
        }
        Err(err) => {
            tracing::warn!(
                ?canvas_id,
                ?err,
                "create_renderer failed; canvas stays as source"
            );
        }
    }
}

/// Write the finalized canvas identified by `canvas_id` to
/// `~/.otto/canvases/<unix>-<turn>-<block>.html`.
///
/// Called immediately after [`create_canvas_renderer`] succeeds so the
/// export is gated on full block reception. Failures are warn-logged;
/// the canvas entry in the conversation log is unaffected. Disable
/// auto-export by toggling the `internal:html-canvas` plugin off via
/// `~/.otto/plugins.toml`.
pub(crate) fn auto_export_canvas(app: &App, canvas_id: otto_plugin::ContentBlockId, turn_id: u32) {
    use crate::app::Entry;
    use crate::plugin::builtin::html_canvas::auto_export::{
        auto_export_path, canvases_dir, write_canvas,
    };

    let Some(base) = canvases_dir() else {
        return;
    };

    // Retrieve the finalized source from the entry.
    let source = match app
        .entries
        .iter()
        .find(|e| matches!(e, Entry::Canvas { id, .. } if *id == canvas_id))
    {
        Some(Entry::Canvas { source, .. }) => source.clone(),
        _ => {
            tracing::debug!(?canvas_id, "auto_export_canvas: canvas entry not found");
            return;
        }
    };

    let unix_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let path = auto_export_path(&base, unix_ts, turn_id, canvas_id);
    if let Err(err) = write_canvas(&path, &source) {
        tracing::warn!(?err, ?path, "auto-export of canvas failed");
    } else {
        tracing::debug!(?path, "canvas auto-exported");
    }
}

/// If `app.context_size` (the chars/4 estimate) has moved since the last
/// emission, fire `HostEvent::ContextSizeChanged` so footer/status
/// plugins can rerender their `~N ctx` segment without polling. Called
/// once per event-loop iteration from `run_app`, just before render.
///
/// Errors are warn-only — a buggy subscriber must not block the loop.
async fn maybe_emit_context_changed(app: &mut App, last_emitted: &mut u32) {
    let current = app.context_size as u32;
    if current == *last_emitted {
        return;
    }
    *last_emitted = current;
    if let Err(err) = crate::plugin::effects::dispatch_host_event(
        app,
        otto_plugin::HostEvent::ContextSizeChanged { tokens: current },
        0,
    )
    .await
    {
        tracing::warn!(error = %err, "ContextSizeChanged dispatch failed");
    }
}

async fn run_app(
    terminal: &mut tui::Tui,
    app: &mut App,
    host_slot: HostSlot,
    project_root: PathBuf,
    tool_bins: ToolBins,
) -> Result<()> {
    let (worker_tx, mut worker_rx) = mpsc::channel::<WorkerMsg>(128);

    // PR 7: HostEvent emission state. These counters live here, not on
    // `App`, because they're a property of the host→plugin event stream
    // and only the event-loop driver knows the right moment to mint a
    // fresh id. They're plain `u32`/`u64`; nothing else mutates them.
    //
    // - `next_turn_id`: incremented at each new turn (the first
    //   `IterationStarted` after `current_turn_id` is `None`).
    // - `current_turn_id`: the id assigned once the host starts the turn,
    //   used to match `TurnStart`/`TurnEnd` payloads. Cleared on terminal
    //   turn outcomes and WorkerMsg::Error.
    // - `footer_pending_turn_id`: predicted next turn id shown in the TUI
    //   footer after prompt submission but before the first
    //   `IterationStarted` arrives. `/bash` does not toggle it.
    // - `next_tool_call_id`: minted per `ToolCallStarted`.
    // - `last_tool_call_id`: tracks the most recent unfinished tool call so
    //   that the matching `ToolCallFinished` emits the same `call_id`.
    //   Tool calls are serialized per-turn inside `run_turn_inner`, so a
    //   single "last" slot is sufficient — no interleaving to worry about.
    // - `last_emitted_ctx`: tracks the most recent `ContextSizeChanged`
    //   payload so we only emit when the value actually moves.
    let mut next_turn_id: u32 = 0;
    let mut current_turn_id: Option<u32> = None;
    let mut current_turn_provider_id: Option<otto_protocol::ProviderId> = None;
    let mut footer_pending_turn_id: Option<u32> = None;
    let mut turn_terminal_event_seen = false;
    let mut next_tool_call_id: u64 = 0;
    let mut last_tool_call_id: Option<u64> = None;
    let mut last_emitted_ctx: u32 = 0;
    let mut render_tick: u64 = 0;

    // Emit `HostEvent::HostStarting` exactly once. Subscribers (e.g.
    // future providers' auto-probe wiring) get one shot at startup.
    if let Err(e) =
        crate::plugin::effects::dispatch_host_event(app, otto_plugin::HostEvent::HostStarting, 0)
            .await
    {
        tracing::warn!(error = %e, "HostStarting dispatch failed");
    }
    // If there is an initial active provider (bootstrap succeeded),
    // notify plugins immediately so their render_slot shows the `▸`
    // marker before the user's first interaction. Without this,
    // `ActiveProviderChanged` would only fire on the first `/use`
    // invocation, leaving the footer unmarked on startup.
    if let Some(initial_pid) = app.active_provider_id {
        match otto_plugin::ProviderId::new(initial_pid) {
            Ok(id) => {
                if let Err(e) = crate::plugin::effects::dispatch_host_event(
                    app,
                    otto_plugin::HostEvent::ActiveProviderChanged { id },
                    0,
                )
                .await
                {
                    tracing::warn!(error = %e, "startup ActiveProviderChanged dispatch failed");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "startup ActiveProviderChanged: invalid provider id");
            }
        }
    }

    // Drain any pool-add queued by HostStarting subscribers (e.g. provider
    // plugins' silent-connect path emitting Effect::RegisterProvider for
    // keyring-stored credentials). Without this, the silent path's
    // Effect::RegisterProvider only updated App::registered_providers and
    // never the host's pool, leaving /model empty and turns unable to
    // route to silently-connected providers. Idempotent w.r.t. providers
    // bootstrap_pool_host already added (apply_pending_pool_add handles
    // PoolError::AlreadyRegistered as a debug no-op).
    apply_pending_pool_add(app, &host_slot, &project_root, &tool_bins, true).await;
    apply_pending_gate(app, &host_slot).await;
    apply_pending_in_process_tools(app, &host_slot).await;
    // `HostStarting` subscribers (e.g. `internal:user-skills`) can populate
    // state that changes their own prompt segment only once this event
    // fires — after the one-shot startup snapshot above main() already
    // pushed to the host. Re-push now so a segment that only exists once
    // discovery has run (the skills catalog) is present for the first
    // turn rather than only after a manual `/reload-*` command.
    apply_pending_prompt_segments_reload(app, &host_slot).await;

    // Populate `App::cached_models` from the bootstrap host's pool so the
    // `/model` picker has rows the moment the user opens it. Previously
    // `cached_models` started empty and was only refreshed by /connect,
    // /use, /model, and perform_model_change — so opening `/model` before
    // any of those ran (the common "I just launched the TUI, providers
    // came up via keyring auto-connect" case) showed the "no models
    // available" placeholder even though the host had a fully-populated
    // pool. No-op when there's no host (e.g. bootstrap returned None
    // because Fix A's startup_connect filter emptied the pool).
    refresh_cached_models(app, &host_slot).await;

    loop {
        // Fire ContextSizeChanged whenever the chars/4 estimate moves so
        // home_footer (and any future status-line subscriber) can refresh
        // its `~N ctx` segment without polling. Cheap when unchanged.
        maybe_emit_context_changed(app, &mut last_emitted_ctx).await;

        let frame_area = terminal.get_frame().area();
        let frame_data = ui::compute_home_frame_data(app, frame_area).await;
        terminal.draw(|f| {
            ui::render(
                app,
                f,
                &frame_data,
                render_tick,
                current_turn_id,
                footer_pending_turn_id,
            )
        })?;
        render_tick = render_tick.wrapping_add(1);

        while let Ok(msg) = worker_rx.try_recv() {
            match msg {
                WorkerMsg::Event(e) => {
                    let was_complete = matches!(e, TurnEvent::TurnComplete { .. });
                    let predicted_turn_id = footer_pending_turn_id;
                    if matches!(
                        &e,
                        TurnEvent::IterationStarted { iteration } if *iteration == 1
                    ) || matches!(
                        e,
                        TurnEvent::TurnComplete { .. }
                            | TurnEvent::Cancelled { .. }
                            | TurnEvent::AbortedAfterGrace { .. }
                    ) {
                        footer_pending_turn_id = None;
                    }
                    if matches!(
                        e,
                        TurnEvent::TurnComplete { .. }
                            | TurnEvent::Cancelled { .. }
                            | TurnEvent::AbortedAfterGrace { .. }
                    ) {
                        turn_terminal_event_seen = true;
                        current_turn_provider_id = None;
                    }
                    // Capture the canvas id before apply_turn_event consumes
                    // the event and removes the index from html_block_index_to_id.
                    let html_block_stop_id = if let TurnEvent::HtmlBlockStop { index } = &e {
                        app.html_block_index_to_id.get(index).copied()
                    } else {
                        None
                    };
                    if let TurnEvent::RouteSelected { provider_id, .. } = &e {
                        current_turn_provider_id = Some(provider_id.clone());
                    }
                    // Translate the streaming TurnEvent before
                    // `apply_turn_event` (which consumes `e` by value)
                    // so the translator and the App mutation each get
                    // their own borrow. The hook fires AFTER
                    // `apply_turn_event` + `update_metrics`, so
                    // subscribers observe the post-mutation App view —
                    // which is what telemetry/render/transcript
                    // subscribers actually want (they need the latest
                    // text buffer, metrics, and entry list).
                    let prestart_cancelled = matches!(
                        &e,
                        TurnEvent::Cancelled { .. } | TurnEvent::AbortedAfterGrace { .. }
                    ) && current_turn_id.is_none()
                        && predicted_turn_id.is_some();
                    let host_event = if prestart_cancelled {
                        None
                    } else {
                        translate_turn_event_to_host_event(
                            &e,
                            &mut next_turn_id,
                            &mut current_turn_id,
                            &mut next_tool_call_id,
                            &mut last_tool_call_id,
                        )
                    };
                    app.apply_turn_event(e);
                    app.update_metrics();
                    // If an HTML block just completed, try to create a renderer
                    // for it via the plugin registry. This is the async half of
                    // handle_html_block_stop — the sync half (preview→source
                    // swap) already happened inside apply_turn_event.
                    if let Some(canvas_id) = html_block_stop_id {
                        create_canvas_renderer(app, canvas_id).await;
                        // Auto-export: write the finalized canvas to
                        // ~/.otto/canvases/<unix>-<turn>-<block>.html.
                        // Runs after renderer creation so it only fires for
                        // fully-received blocks. Disable by toggling the
                        // internal:html-canvas plugin off via plugins.toml.
                        auto_export_canvas(app, canvas_id, current_turn_id.unwrap_or(next_turn_id));
                    }
                    if prestart_cancelled {
                        let turn_id = predicted_turn_id.expect("checked is_some above");
                        next_turn_id = next_turn_id.max(turn_id);
                        last_tool_call_id = None;
                        for host_event in [
                            otto_plugin::HostEvent::TurnStart { turn_id },
                            otto_plugin::HostEvent::TurnEnd {
                                turn_id,
                                success: false,
                            },
                        ] {
                            if let Err(err) =
                                crate::plugin::effects::dispatch_host_event(app, host_event, 0)
                                    .await
                            {
                                tracing::warn!(error = %err,
                                    "host-event dispatch (from TurnEvent) failed");
                            }
                        }
                    } else if let Some(he) = host_event {
                        if let Err(err) =
                            crate::plugin::effects::dispatch_host_event(app, he, 0).await
                        {
                            tracing::warn!(error = %err,
                                "host-event dispatch (from TurnEvent) failed");
                        }
                    }
                    if was_complete {
                        if let Some(host) = current_host(&host_slot).await {
                            if let Ok(path) = save_transcript_now(app, &host).await {
                                if !path.as_os_str().is_empty() {
                                    let saved_path = path.to_string_lossy().into_owned();
                                    app.last_transcript = Some(path);
                                    if let Err(err) = crate::plugin::effects::dispatch_host_event(
                                        app,
                                        otto_plugin::HostEvent::TranscriptSaved {
                                            path: saved_path,
                                        },
                                        0,
                                    )
                                    .await
                                    {
                                        tracing::warn!(error = %err,
                                            "TranscriptSaved dispatch failed");
                                    }
                                }
                            }
                        }
                    }
                }
                WorkerMsg::Error(msg) => {
                    record_turn_error(
                        app,
                        msg,
                        &mut footer_pending_turn_id,
                        &mut current_turn_id,
                        &mut next_turn_id,
                        &mut last_tool_call_id,
                        &mut turn_terminal_event_seen,
                    )
                    .await;
                    current_turn_provider_id = None;
                }
                WorkerMsg::TurnAuthError {
                    message,
                    provider_display_name,
                } => {
                    record_turn_error(
                        app,
                        message,
                        &mut footer_pending_turn_id,
                        &mut current_turn_id,
                        &mut next_turn_id,
                        &mut last_tool_call_id,
                        &mut turn_terminal_event_seen,
                    )
                    .await;
                    if let Some(hint) = crate::providers::turn_auth_hint(
                        current_turn_provider_id.as_ref(),
                        &provider_display_name,
                    ) {
                        app.push_note(hint);
                    }
                    current_turn_provider_id = None;
                }
                WorkerMsg::BashDone => {
                    app.is_loading = false;
                    app.update_metrics();
                }
                WorkerMsg::DisconnectCompleted { provider, mode } => {
                    app.push_note(
                        rust_i18n::t!("notes.disconnect-completed", name = provider, mode = mode)
                            .to_string(),
                    );
                }
                WorkerMsg::DisconnectFailed { provider, err } => {
                    app.push_note(
                        rust_i18n::t!("notes.disconnect-worker-failed", name = provider, err = err)
                            .to_string(),
                    );
                }
                WorkerMsg::ModelRestored(original) => {
                    // Sync app.model back to the original after a one-turn
                    // model override so the status bar shows the correct
                    // (restored) model id.
                    app.model = original;
                }
            }
        }

        if app.should_quit {
            // Close either the active turn or a prompt-submitted turn that
            // has not reached `IterationStarted` yet before tearing down.
            dispatch_failed_turn_end_on_exit(
                app,
                &mut current_turn_id,
                &mut footer_pending_turn_id,
                "quit",
            )
            .await;
            drain_pending_bash_net(app, &host_slot).await;
            return Ok(());
        }

        if app.show_splash && app.splash_shown_at.elapsed() >= splash::SPLASH_DURATION {
            app.show_splash = false;
        }

        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        let evt = event::read()?;
        // Mouse wheel ticks scroll the conversation log when the home screen
        // is active (no plugin screen on top, no modal/file-picker). A left
        // click on a rendered canvas focuses it and routes a synthetic press
        // (and the matching release) into the renderer. Other mouse events
        // are ignored — see `log_scroll_offset_after_wheel` for the offset
        // math.
        if let Event::Mouse(me) = &evt {
            use crossterm::event::{MouseButton as CtMouseButton, MouseEventKind as MEK};
            // Scroll-wheel: only on the home editing screen.
            let on_home = app.screen_stack.is_empty()
                && matches!(app.input_mode, InputMode::Editing)
                && !app.is_file_picker_active
                && !app.show_splash;
            if on_home {
                let dir = match me.kind {
                    MEK::ScrollUp => Some(app::WheelDirection::Up),
                    MEK::ScrollDown => Some(app::WheelDirection::Down),
                    _ => None,
                };
                if let Some(direction) = dir {
                    app.log_scroll_offset_from_bottom = app::log_scroll_offset_after_wheel(
                        app.log_scroll_offset_from_bottom,
                        direction,
                        MOUSE_WHEEL_SCROLL_STEP,
                    );
                }
            }
            // Canvas click routing. Allowed whenever no plugin screen /
            // file-picker / splash is up — including while already focused on
            // a canvas (`InputMode::Canvas`), so clicks can move between
            // canvases. Click-only for Phase 2.0: we route Press/Release but
            // NOT Move/Drag, because each dispatch re-parses the document and
            // pointer-move spam would tank performance. `a:hover` is therefore
            // not supported yet; revisit if hover becomes a requirement.
            let canvas_routable = app.screen_stack.is_empty()
                && !app.is_file_picker_active
                && !app.show_splash
                && app.canvas_registry.image_protocol_available();
            let portable_kind = match me.kind {
                MEK::Down(_) => Some(otto_plugin::MouseEventKind::Press),
                MEK::Up(_) => Some(otto_plugin::MouseEventKind::Release),
                _ => None,
            };
            if canvas_routable {
                if let Some(kind) = portable_kind {
                    if let Some((cw, ch)) = app.canvas_registry.image_cell_size() {
                        let cell = otto_canvas::CellPixelSize {
                            width: cw,
                            height: ch,
                        };
                        if let Some((cid, px, py)) =
                            app::canvas_hit(&app.canvas_click_targets, me.column, me.row, cell)
                        {
                            // A press moves focus into the clicked canvas
                            // before the event is delivered, so the renderer
                            // sees the click as the focused element.
                            if matches!(kind, otto_plugin::MouseEventKind::Press)
                                && !app.is_canvas_focused(cid)
                            {
                                app.focus_canvas(cid, None);
                            }
                            let button = match me.kind {
                                MEK::Down(b) | MEK::Up(b) | MEK::Drag(b) => Some(match b {
                                    CtMouseButton::Left => otto_plugin::MouseButton::Left,
                                    CtMouseButton::Right => otto_plugin::MouseButton::Right,
                                    CtMouseButton::Middle => otto_plugin::MouseButton::Middle,
                                }),
                                _ => None,
                            };
                            let portable = otto_plugin::MouseEventPortable {
                                kind,
                                button,
                                x_pixel: px,
                                y_pixel: py,
                                modifiers: crate::plugin::convert::modifiers_to_portable(
                                    me.modifiers,
                                ),
                            };
                            let _ = crate::canvas_input::handle_canvas_mouse(
                                app, &host_slot, cid, portable,
                            )
                            .await;
                        }
                    }
                }
            }
            continue;
        }
        let Event::Key(key) = &evt else { continue };
        if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
            continue;
        }
        let top_screen_id = app.screen_stack.top_id();
        if key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && global_quit_allowed(top_screen_id)
        {
            dispatch_failed_turn_end_on_exit(
                app,
                &mut current_turn_id,
                &mut footer_pending_turn_id,
                "ctrl-c",
            )
            .await;
            drain_pending_bash_net(app, &host_slot).await;
            return Ok(());
        }

        if app.show_splash {
            app.show_splash = false;
            continue;
        }

        // Screen-stack routing: if any screen is on top, route the key there.
        // Reserved shortcuts (Ctrl-C already caught above; Ctrl-D quits) are
        // handled before forwarding to the screen.
        //
        // PR 3 limitation: KeyScope::OnScreen keybinding contributions are not
        // dispatched here. When a screen is on top, key events route directly
        // to its on_key(). PR 6 or later may add a KeybindingRouter::route
        // pass with active_screen=Some(top.id()) before falling through to
        // on_key, once a built-in screen actually needs OnScreen bindings.
        if !app.screen_stack.is_empty() {
            let portable = crate::plugin::convert::key_event_to_portable(*key);
            if portable.modifiers.ctrl
                && matches!(portable.code, otto_plugin::KeyCodePortable::Char('d'))
                && global_quit_allowed(top_screen_id)
            {
                dispatch_failed_turn_end_on_exit(
                    app,
                    &mut current_turn_id,
                    &mut footer_pending_turn_id,
                    "ctrl-d",
                )
                .await;
                drain_pending_bash_net(app, &host_slot).await;
                return Ok(());
            }
            let effs = {
                let (top_screen, _layout) =
                    app.screen_stack.top_mut().expect("just checked non-empty");
                match top_screen.on_key(portable).await {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!(error = %e, "screen on_key error");
                        continue;
                    }
                }
            };
            if let Err(e) = crate::plugin::effects::apply_effects(app, effs).await {
                tracing::warn!(error = %e, "apply_effects from screen failed");
            }
            apply_pending_model_change(app, &host_slot, &project_root, &tool_bins).await;
            apply_pending_pool_add(app, &host_slot, &project_root, &tool_bins, false).await;
            apply_pending_gate(app, &host_slot).await;
            apply_pending_in_process_tools(app, &host_slot).await;
            apply_pending_routing_reload(app, &host_slot).await;
            apply_pending_routing_show(app, &host_slot).await;
            apply_pending_prompt_segments_reload(app, &host_slot).await;
            continue;
        }

        match app.input_mode {
            InputMode::Editing => {
                if app.is_file_picker_active {
                    match key.code {
                        KeyCode::Enter => {
                            let file = app.file_explorer.current();
                            if file.is_dir {
                                app.file_explorer.handle(&evt)?;
                            } else {
                                app.file_picker_select();
                            }
                        }
                        KeyCode::Esc => app.close_file_picker(),
                        _ => {
                            app.file_explorer.handle(&evt)?;
                        }
                    }
                } else {
                    match key.code {
                        // Shift+Enter inserts a newline in the prompt. Only
                        // terminals that speak the Kitty keyboard protocol
                        // (pushed in `tui::init`) report the SHIFT modifier
                        // on Enter — on others, Shift+Enter is
                        // indistinguishable from Enter and the arm below
                        // submits. Documented as a tradeoff in `tui.rs`.
                        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                            app.input_textarea.insert_newline();
                        }
                        // Ctrl+Z → undo, Ctrl+Y → redo. Mirrors the
                        // Windows/Linux desktop convention. tui-textarea
                        // ships undo on Ctrl+U / redo on Ctrl+R (which
                        // still work via fallthrough); Ctrl+Y otherwise
                        // pastes in tui-textarea, but desktop muscle
                        // memory of Ctrl+Y=redo wins here.
                        KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.input_textarea.undo();
                        }
                        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.input_textarea.redo();
                        }
                        KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                            let value = app.input_textarea.lines().join("\n");
                            if value.is_empty() || app.is_loading {
                                continue;
                            }
                            // Submitting always returns the user to the live
                            // tail — they want to see what they just sent and
                            // the streaming response, not the rows they had
                            // scrolled back to.
                            app.log_scroll_offset_from_bottom = None;
                            // Record every submission (slash commands too — they
                            // were typed at the prompt and the user expects Up to
                            // recall them like a shell). `append` no-ops on empty
                            // input and dedupes consecutive duplicates.
                            app.prompt_history.append(value.clone());
                            if value.starts_with('/') {
                                app.input_textarea = make_input_textarea(Vec::<String>::new());
                                dispatch_slash_command(
                                    app,
                                    &value,
                                    &host_slot,
                                    &project_root,
                                    &tool_bins,
                                    &worker_tx,
                                )
                                .await;
                                continue;
                            }
                            let Some(host) = current_host(&host_slot).await else {
                                app.push_note(
                                    rust_i18n::t!("notes.not-connected-connect-first").to_string(),
                                );
                                app.input_textarea = make_input_textarea(Vec::<String>::new());
                                continue;
                            };
                            app.push_user(value.clone());
                            app.input_textarea = make_input_textarea(Vec::<String>::new());
                            app.is_loading = true;
                            turn_terminal_event_seen = false;
                            // Fire HostEvent::PromptSubmitted so hook
                            // subscribers (transcript loggers, telemetry,
                            // future custom prompt-rewriters) see the
                            // submission before the host begins streaming
                            // turn events. Errors are warn-only — a buggy
                            // subscriber must not block the turn.
                            if let Err(err) = crate::plugin::effects::dispatch_host_event(
                                app,
                                otto_plugin::HostEvent::PromptSubmitted {
                                    text: value.clone(),
                                },
                                0,
                            )
                            .await
                            {
                                tracing::warn!(error = %err,
                                    "PromptSubmitted dispatch failed");
                            }

                            // Drain hook-driven prompt mutations now that
                            // PromptSubmitted dispatch has completed.
                            // CancelPendingTurn takes precedence over
                            // PrependToPendingPrompt — if a hook blocked the
                            // turn, drop the accumulated prefix too so it
                            // doesn't bleed into a future turn.
                            if let Some(reason) = app.pending_turn_cancellation.take() {
                                app.push_note(format!("[blocked] {reason}"));
                                app.is_loading = false;
                                app.pending_prompt_prefix = None;
                                continue;
                            }
                            footer_pending_turn_id = Some(next_turn_id.saturating_add(1));
                            let prefix = app.pending_prompt_prefix.take();

                            // Consume the one-turn model override (if any)
                            // before moving `app` references into the spawn.
                            // `original_model` is saved so the host can be
                            // restored after the turn completes.
                            let model_override = app.consume_model_override();
                            let original_model = app.model.clone();

                            // Build the prompt text that goes to the host.
                            // The hook-supplied prefix MUST NOT appear in
                            // `push_user` / `prompt_history.append` /
                            // `PromptSubmitted` (all of which already ran on
                            // `value`); it only shapes the prompt the host
                            // sees, so the user's transcript and history
                            // stay clean.
                            let prompt_text = match prefix {
                                Some(p) => format!("{p}\n\n{value}"),
                                None => value.clone(),
                            };

                            let tx = worker_tx.clone();
                            tokio::spawn(async move {
                                // Apply the per-turn model override to the host
                                // before the turn starts. The host's `current_model`
                                // is restored unconditionally after the turn
                                // (success or error) so subsequent turns are
                                // unaffected.
                                if let Some(ref override_id) = model_override {
                                    tracing::debug!(
                                        model = %override_id,
                                        "applying one-turn model override"
                                    );
                                    host.set_model(override_id.clone()).await;
                                }

                                let (ev_tx, mut ev_rx) = mpsc::channel(64);
                                let host_for_run = host.clone();
                                let prompt = prompt_text;
                                let runner = tokio::spawn(async move {
                                    host_for_run.run_turn_streaming(prompt, ev_tx).await
                                });
                                while let Some(ev) = ev_rx.recv().await {
                                    if tx.send(WorkerMsg::Event(ev)).await.is_err() {
                                        break;
                                    }
                                }
                                let result = runner.await;

                                // Restore the original model on the host after
                                // the turn, whether it succeeded or failed. Only
                                // needed when an override was actually applied.
                                if model_override.is_some() {
                                    tracing::debug!(
                                        model = %original_model,
                                        "restoring model after one-turn override"
                                    );
                                    host.set_model(original_model.clone()).await;
                                    // Notify the main loop so app.model stays in
                                    // sync with host.current_model.
                                    let _ = tx.send(WorkerMsg::ModelRestored(original_model)).await;
                                }

                                match result {
                                    Ok(Ok(_)) => {}
                                    Ok(Err(e)) => {
                                        let message = e.to_string();
                                        let auth_name =
                                            if let otto_host::HostError::Provider { name, error } =
                                                &e
                                            {
                                                (error.kind
                                                    == otto_protocol::ErrorKind::Authentication)
                                                    .then(|| name.clone())
                                            } else {
                                                None
                                            };
                                        match auth_name {
                                            Some(provider_display_name) => {
                                                let _ = tx
                                                    .send(WorkerMsg::TurnAuthError {
                                                        message,
                                                        provider_display_name,
                                                    })
                                                    .await;
                                            }
                                            None => {
                                                let _ = tx.send(WorkerMsg::Error(message)).await;
                                            }
                                        }
                                    }
                                    Err(join_err) => {
                                        let _ = tx
                                            .send(WorkerMsg::Error(format!(
                                                "worker task failed: {join_err}"
                                            )))
                                            .await;
                                    }
                                }
                            });
                        }
                        KeyCode::Esc => {
                            app.input_textarea = make_input_textarea(Vec::<String>::new());
                            // Treat Esc as "back to a clean state" — the
                            // input is wiped, so the conversation log
                            // returns to the live tail too. Matches the
                            // contract documented on `log_scroll_offset_from_bottom`.
                            app.log_scroll_offset_from_bottom = None;
                        }
                        KeyCode::PageUp => {
                            let next = app
                                .log_scroll_offset_from_bottom
                                .unwrap_or(0)
                                .saturating_add(LOG_SCROLL_STEP);
                            app.log_scroll_offset_from_bottom = Some(next);
                        }
                        KeyCode::PageDown => {
                            app.log_scroll_offset_from_bottom = app
                                .log_scroll_offset_from_bottom
                                .and_then(|n| n.checked_sub(LOG_SCROLL_STEP))
                                .filter(|n| *n > 0);
                        }
                        // Home/End on the home screen scroll the conversation
                        // log only when the textarea is empty or Ctrl is held.
                        // Otherwise the keystroke falls through to tui-textarea
                        // so cursor-to-line-start / line-end still work during
                        // editing.
                        KeyCode::Home
                            if key.modifiers.contains(KeyModifiers::CONTROL)
                                || app.input_textarea.lines().iter().all(|l| l.is_empty()) =>
                        {
                            app.log_scroll_offset_from_bottom = Some(u16::MAX);
                        }
                        KeyCode::End
                            if key.modifiers.contains(KeyModifiers::CONTROL)
                                || app.input_textarea.lines().iter().all(|l| l.is_empty()) =>
                        {
                            app.log_scroll_offset_from_bottom = None;
                        }
                        KeyCode::Char('@') => {
                            app.input_textarea.input(evt);
                            app.open_file_picker();
                        }
                        // Up/Down at the prompt: recall previous/next history
                        // entry when the input is empty (Up) or while an active
                        // browse's marker still matches the textarea content.
                        // Any modifier (Shift/Ctrl/Alt) falls through so chorded
                        // bindings and future selection support are untouched.
                        KeyCode::Up if key.modifiers.is_empty() => {
                            let current = app.input_textarea.lines().join("\n");
                            if let Some(text) = app.prompt_history.recall_prev(&current) {
                                app.set_input_text_for_history(&text);
                            } else {
                                app.input_textarea.input(evt);
                            }
                        }
                        KeyCode::Down if key.modifiers.is_empty() => {
                            let current = app.input_textarea.lines().join("\n");
                            if let Some(text) = app.prompt_history.recall_next(&current) {
                                app.set_input_text_for_history(&text);
                            } else {
                                app.input_textarea.input(evt);
                            }
                        }
                        _ => {
                            // Try keybinding router (OnHome → Global) before
                            // falling through to the textarea. This is what
                            // fires `/` → OpenScreen("palette") when the
                            // prompt is empty.
                            let portable = crate::plugin::convert::key_event_to_portable(*key);
                            let mut handled = false;
                            if let (Some(_reg), Some(idx)) =
                                (&app.plugin_registry, &app.plugin_indexes)
                            {
                                let action = {
                                    let idx_guard = idx.read().await;
                                    let router = crate::plugin::keybindings::KeybindingRouter::new(
                                        &idx_guard,
                                    );
                                    should_route_home_keybinding(key, app.input_textarea.lines())
                                        .then(|| router.route(&portable, None))
                                        .flatten()
                                };
                                if let Some(action) = action {
                                    dispatch_bound_action(app, action).await;
                                    apply_pending_model_change(
                                        app,
                                        &host_slot,
                                        &project_root,
                                        &tool_bins,
                                    )
                                    .await;
                                    apply_pending_pool_add(
                                        app,
                                        &host_slot,
                                        &project_root,
                                        &tool_bins,
                                        false,
                                    )
                                    .await;
                                    apply_pending_gate(app, &host_slot).await;
                                    apply_pending_in_process_tools(app, &host_slot).await;
                                    apply_pending_routing_reload(app, &host_slot).await;
                                    apply_pending_routing_show(app, &host_slot).await;
                                    apply_pending_prompt_segments_reload(app, &host_slot).await;
                                    handled = true;
                                }
                            }
                            if !handled {
                                app.input_textarea.input(evt);
                            }
                        }
                    }
                }
            }
            InputMode::SelectingProvider => {
                if let Some(connect) = handle_provider_selector_key(app, *key, |spec| {
                    creds::load(spec.id).map_err(|e| format!("{e:#}"))
                }) {
                    perform_connect(
                        connect.spec,
                        connect.api_key,
                        &host_slot,
                        &project_root,
                        &tool_bins,
                        app,
                    )
                    .await;
                }
            }
            InputMode::EnteringApiKey => match key.code {
                KeyCode::Esc => app.cancel_connect(),
                KeyCode::Enter => {
                    let action = handle_api_key_modal_submit(app, |spec| {
                        creds::load(spec.id).map_err(|e| format!("{e:#}"))
                    });
                    match action {
                        ApiKeySubmitAction::Connect { spec, api_key } => {
                            perform_connect(
                                spec,
                                api_key,
                                &host_slot,
                                &project_root,
                                &tool_bins,
                                app,
                            )
                            .await;
                        }
                        ApiKeySubmitAction::NoStoredKey | ApiKeySubmitAction::Idle => {}
                    }
                }
                _ => {
                    app.api_key_textarea.input(evt);
                }
            },
            InputMode::PermissionPrompt => {
                let action = match key.code {
                    KeyCode::Char('y') => Some((PermissionDecision::Allow, false)),
                    KeyCode::Char('n') | KeyCode::Esc => Some((PermissionDecision::Deny, false)),
                    KeyCode::Char('a') => Some((PermissionDecision::Allow, true)),
                    KeyCode::Char('N') => Some((PermissionDecision::Deny, true)),
                    _ => None,
                };
                if let Some((decision, persist)) = action {
                    resolve_pending_permission(app, &host_slot, decision, persist).await;
                }
            }
            InputMode::BashNetworkPrompt { id, .. } => {
                let choice = match key.code {
                    KeyCode::Char('o') | KeyCode::Char('O') => Some(BashNetworkChoice::Once),
                    KeyCode::Char('a') | KeyCode::Char('A') => {
                        Some(BashNetworkChoice::AlwaysThisSession)
                    }
                    KeyCode::Char('d') | KeyCode::Char('D') => Some(BashNetworkChoice::DenyOnce),
                    KeyCode::Char('f')
                    | KeyCode::Char('F')
                    | KeyCode::Char('n')
                    | KeyCode::Char('N') => Some(BashNetworkChoice::DenyAlways),
                    // Esc → Cancelled (policy-equivalent to DenyOnce, but
                    // labelled distinctly so the user sees that backing
                    // out implied a deny rather than reading their Esc
                    // as an active "deny" decision.
                    KeyCode::Esc => Some(BashNetworkChoice::Cancelled),
                    _ => None,
                };
                if let Some(choice) = choice {
                    if let Some(host) = current_host(&host_slot).await {
                        let host = host.clone();
                        tokio::spawn(async move {
                            host.resolve_bash_network_decision(id, choice).await;
                        });
                    }
                    app.input_mode = InputMode::Editing;
                    let label = match choice {
                        BashNetworkChoice::Once => {
                            rust_i18n::t!("bash.net-allowed-once").to_string()
                        }
                        BashNetworkChoice::AlwaysThisSession => {
                            rust_i18n::t!("bash.net-always-allowed").to_string()
                        }
                        BashNetworkChoice::DenyOnce => {
                            rust_i18n::t!("bash.net-denied-once").to_string()
                        }
                        BashNetworkChoice::DenyAlways => {
                            rust_i18n::t!("bash.net-never").to_string()
                        }
                        BashNetworkChoice::Cancelled => {
                            rust_i18n::t!("bash.net-cancelled").to_string()
                        }
                    };
                    app.push_note(label);
                }
            }
            InputMode::SelectingTranscript => match key.code {
                KeyCode::Esc => app.close_transcript_picker(),
                KeyCode::Up if app.transcript_index > 0 => {
                    app.transcript_index -= 1;
                }
                KeyCode::Down => {
                    let count = app.transcript_entries.len();
                    if app.transcript_index + 1 < count {
                        app.transcript_index += 1;
                    }
                }
                KeyCode::Enter => {
                    if let Some(path) = app.selected_transcript_path().map(|p| p.to_path_buf()) {
                        app.close_transcript_picker();
                        if app.is_loading {
                            app.push_note(
                                rust_i18n::t!("notes.cannot-resume-during-turn").to_string(),
                            );
                        } else {
                            do_resume_from_path(app, &host_slot, &path).await;
                        }
                    }
                }
                _ => {}
            },
            // Canvas focus: built-in keys (Esc / Tab / BackTab / Ctrl-J /
            // Ctrl-K / Ctrl-O) take precedence over plugin
            // `OnFocusedCanvas` bindings, which in turn precede a raw key
            // dispatch to the renderer. See
            // `canvas_input::handle_focused_canvas_key`.
            InputMode::Canvas { id, element_idx } => {
                let portable = crate::plugin::convert::key_event_to_portable(*key);
                crate::canvas_input::handle_focused_canvas_key(
                    app,
                    &host_slot,
                    id,
                    element_idx,
                    portable,
                )
                .await;
            }
        }
    }
}

/// Dispatch a [`BoundAction`][otto_plugin::BoundAction] produced by the
/// keybinding router. Logs and surfaces errors to the user via
/// `push_styled_note` so a malformed binding or runtime error doesn't
/// silently no-op a keystroke.
pub(crate) async fn dispatch_bound_action(app: &mut App, action: otto_plugin::BoundAction) {
    match action {
        otto_plugin::BoundAction::EmitEffect(effect) => {
            if let Err(e) = crate::plugin::effects::apply_effects(app, vec![effect]).await {
                tracing::warn!(error = %e, "apply_effects from keybinding failed");
                app.push_styled_note(otto_plugin::StyledLine::plain(format!(
                    "Action failed: {e}"
                )));
            }
        }
        otto_plugin::BoundAction::RunSlash { name, args } => {
            let (reg, idx) = match (&app.plugin_registry, &app.plugin_indexes) {
                (Some(r), Some(i)) => (r.clone(), i.clone()),
                _ => {
                    tracing::warn!("dispatch_bound_action: plugin runtime not installed");
                    return;
                }
            };
            let effs_result = {
                let router = crate::plugin::slash::SlashRouter::new(idx.clone(), reg.clone());
                router.dispatch(&name, args).await
            };
            match effs_result {
                Ok(effs) => {
                    if let Err(e) = crate::plugin::effects::apply_effects(app, effs).await {
                        tracing::warn!(error = %e, command = %name, "apply_effects after slash dispatch failed");
                        app.push_styled_note(otto_plugin::StyledLine::plain(
                            rust_i18n::t!("notes.command-failed", err = format!("{e:#}"))
                                .to_string(),
                        ));
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, command = %name, "slash dispatch failed");
                    app.push_styled_note(otto_plugin::StyledLine::plain(
                        rust_i18n::t!("notes.command-failed", err = format!("{e:#}")).to_string(),
                    ));
                }
            }
        }
        _ => {}
    }
}

#[derive(Debug, PartialEq, Eq)]
struct PendingProviderConnect {
    spec: &'static ProviderSpec,
    api_key: String,
}

fn submit_selected_provider<F>(app: &mut App, mut load_creds: F) -> Option<PendingProviderConnect>
where
    F: FnMut(&'static ProviderSpec) -> Result<Option<String>, String>,
{
    let spec = app.selected_provider()?;
    if !spec.api_key_required {
        app.input_mode = InputMode::Editing;
        return Some(PendingProviderConnect {
            spec,
            api_key: String::new(),
        });
    }

    match load_creds(spec) {
        Ok(Some(_)) => {
            // A credential is already stored — open the modal instead of
            // connecting immediately. Defense-in-depth: this whole function
            // is a legacy fallback, unreachable while the Core
            // internal:connect plugin is installed (see the "/connect" arm
            // of `App::handle_command` in `app.rs`), but it should stay
            // consistent with the live plugin path. See savvagent/otto#146.
            app.enter_api_key_for(spec, true);
            None
        }
        Ok(None) => {
            app.enter_api_key_for(spec, false);
            None
        }
        Err(err) => {
            app.push_note(rust_i18n::t!("notes.keyring-error", err = err).to_string());
            app.enter_api_key_for(spec, false);
            None
        }
    }
}

fn handle_provider_selector_key<F>(
    app: &mut App,
    key: event::KeyEvent,
    load_creds: F,
) -> Option<PendingProviderConnect>
where
    F: FnMut(&'static ProviderSpec) -> Result<Option<String>, String>,
{
    match key.code {
        KeyCode::Esc if app.provider_query.is_empty() => {
            app.input_mode = InputMode::Editing;
            None
        }
        KeyCode::Esc => {
            app.clear_provider_query();
            None
        }
        KeyCode::Up if app.provider_index > 0 => {
            app.provider_index -= 1;
            None
        }
        KeyCode::Down if app.provider_index + 1 < app.filtered_providers().len() => {
            app.provider_index += 1;
            None
        }
        KeyCode::Backspace if !app.provider_query.is_empty() => {
            app.pop_provider_query();
            None
        }
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
        {
            let mut query = app.provider_query.clone();
            query.push(c);
            app.set_provider_query(query);
            None
        }
        KeyCode::Enter => submit_selected_provider(app, load_creds),
        _ => None,
    }
}

enum ApiKeySubmitAction {
    /// Connect using this key (freshly typed, or the reused stored one).
    Connect {
        spec: &'static ProviderSpec,
        api_key: String,
    },
    /// Empty submit, no stored key to fall back to — stayed in the modal.
    NoStoredKey,
    /// No modal was open; caller should ignore.
    Idle,
}

// Hand-written rather than `#[derive(Debug)]`: `Connect`'s `api_key` is a live credential, and a
// derived impl would happily print it verbatim from any future `{:?}`/`dbg!()` call site (a log
// line, a panic message) that isn't this file's own test assertions. Redact it explicitly so that
// mistake can't leak a real API key.
impl std::fmt::Debug for ApiKeySubmitAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiKeySubmitAction::Connect { spec, .. } => f
                .debug_struct("Connect")
                .field("spec", &spec.id)
                .field("api_key", &"<redacted>")
                .finish(),
            ApiKeySubmitAction::NoStoredKey => write!(f, "NoStoredKey"),
            ApiKeySubmitAction::Idle => write!(f, "Idle"),
        }
    }
}

/// Handle `Enter` inside the API-key modal (`InputMode::EnteringApiKey`).
///
/// Three outcomes, mirroring `App::take_pending_api_key`'s own three
/// outcomes: a typed key always wins and replaces whatever was stored; an
/// empty submit falls back to the stored credential when one exists (the
/// one-keystroke "use stored key" path the modal's placeholder
/// advertises); an empty submit with nothing stored leaves the modal open.
fn handle_api_key_modal_submit<F>(app: &mut App, mut load_creds: F) -> ApiKeySubmitAction
where
    F: FnMut(&'static ProviderSpec) -> Result<Option<String>, String>,
{
    match app.take_pending_api_key() {
        Some((spec, Some(key))) => {
            app.input_mode = InputMode::Editing;
            ApiKeySubmitAction::Connect { spec, api_key: key }
        }
        Some((spec, None)) => match load_creds(spec) {
            Ok(Some(stored)) => {
                app.cancel_connect();
                app.push_note(
                    rust_i18n::t!("notes.using-stored-key", name = spec.display_name).to_string(),
                );
                ApiKeySubmitAction::Connect {
                    spec,
                    api_key: stored,
                }
            }
            Ok(None) => {
                app.push_note(rust_i18n::t!("notes.api-key-empty").to_string());
                ApiKeySubmitAction::NoStoredKey
            }
            Err(err) => {
                // Distinguish "keyring read failed" from "nothing stored" —
                // mirrors submit_selected_provider's Err(err) arm, so a
                // backend error isn't silently presented as an empty
                // keyring.
                app.push_note(rust_i18n::t!("notes.keyring-error", err = err).to_string());
                ApiKeySubmitAction::NoStoredKey
            }
        },
        None => ApiKeySubmitAction::Idle,
    }
}

/// On graceful exit, if the user is mid-modal on a bash-network prompt,
/// resolve it as [`BashNetworkChoice::DenyOnce`] so any worker awaiting
/// the corresponding `oneshot` doesn't hang while the runtime tears
/// down. Without this, the worker would only unblock once the `Host`
/// (and the Sender inside `pending_bash_network`) is finally dropped —
/// which happens *after* `run_app` returns. That gap is small but real,
/// and on shutdown we want the worker to finish promptly and let
/// `Host::shutdown` drain cleanly.
async fn drain_pending_bash_net(app: &App, host_slot: &HostSlot) {
    if let InputMode::BashNetworkPrompt { id, .. } = app.input_mode {
        if let Some(host) = current_host(host_slot).await {
            host.resolve_bash_network_decision(id, BashNetworkChoice::DenyOnce)
                .await;
        }
    }
}

/// Pop the pending permission off the app and resolve it on the active
/// host. With `persist = true`, also records a session rule so future
/// requests with identical args short-circuit the modal.
async fn resolve_pending_permission(
    app: &mut App,
    host_slot: &HostSlot,
    decision: PermissionDecision,
    persist: bool,
) {
    let Some(req) = app.pending_permission.take() else {
        app.input_mode = InputMode::Editing;
        return;
    };
    let host = current_host(host_slot).await;
    if let Some(host) = &host {
        if persist {
            host.add_session_rule(&req.name, &req.args, decision).await;
        }
        host.resolve_permission(req.id, decision).await;
    } else {
        // Host swapped while modal was up — the old host's gate will return
        // Err on its dropped oneshot, which is the cleanup path. Nothing
        // for us to do here.
    }
    app.input_mode = InputMode::Editing;

    let label = match (decision, persist) {
        (PermissionDecision::Allow, false) => rust_i18n::t!("permission.allowed-once").to_string(),
        (PermissionDecision::Allow, true) => rust_i18n::t!("permission.always-allowed").to_string(),
        (PermissionDecision::Deny, false) => rust_i18n::t!("permission.denied").to_string(),
        (PermissionDecision::Deny, true) => rust_i18n::t!("permission.always-denied").to_string(),
    };
    app.push_note(format!("{}: {label}", req.name));
}

#[cfg(test)]
mod model_validation_tests {
    use super::{ModelChangeOutcome, resolve_model_change, validate_model_id};
    use otto_host::{ListModelsResponse, ModelInfo};
    use otto_protocol::{ErrorKind, ProviderError};

    fn info(id: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            display_name: None,
            context_window: None,
        }
    }

    fn resp(ids: &[&str]) -> ListModelsResponse {
        ListModelsResponse {
            models: ids.iter().map(|id| info(id)).collect(),
            default_model_id: None,
        }
    }

    fn err(kind: ErrorKind, msg: &str) -> ProviderError {
        ProviderError {
            kind,
            message: msg.to_string(),
            retry_after_ms: None,
            provider_code: None,
        }
    }

    #[test]
    fn validate_model_id_known() {
        let models = vec![info("a"), info("b")];
        assert!(validate_model_id("a", &models).is_ok());
    }

    #[test]
    fn validate_model_id_unknown_returns_known_set() {
        let models = vec![info("a"), info("b")];
        let err = validate_model_id("c", &models).unwrap_err();
        assert_eq!(err, vec!["a", "b"]);
    }

    #[test]
    fn validate_model_id_empty_list_always_rejects() {
        let models: Vec<ModelInfo> = vec![];
        let err = validate_model_id("anything", &models).unwrap_err();
        assert!(err.is_empty());
    }

    #[test]
    fn resolve_proceeds_silently_for_known_id() {
        let r = resp(&["a", "b"]);
        let outcome = resolve_model_change("a", Ok(&r));
        assert_eq!(outcome, ModelChangeOutcome::Proceed { warning: None });
    }

    #[test]
    fn resolve_rejects_unknown_id_with_known_set() {
        use crate::test_helpers::HOME_LOCK;
        let _lock = HOME_LOCK.lock().unwrap();
        rust_i18n::set_locale("en");

        let r = resp(&["a", "b"]);
        let outcome = resolve_model_change("c", Ok(&r));
        match outcome {
            ModelChangeOutcome::Reject { note } => {
                assert!(note.contains("Unknown model `c`"), "note: {note}");
                assert!(note.contains("a, b"), "note: {note}");
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn resolve_empty_list_proceeds_with_warning() {
        use crate::test_helpers::HOME_LOCK;
        let _lock = HOME_LOCK.lock().unwrap();
        rust_i18n::set_locale("en");

        let r = resp(&[]);
        let outcome = resolve_model_change("anything", Ok(&r));
        match outcome {
            ModelChangeOutcome::Proceed { warning: Some(w) } => {
                assert!(w.contains("Provider advertises no models"), "w: {w}");
                assert!(w.contains("`anything`"), "w: {w}");
            }
            other => panic!("expected Proceed with warning, got {other:?}"),
        }
    }

    #[test]
    fn resolve_not_implemented_proceeds_silently() {
        let e = err(ErrorKind::NotImplemented, "list_models not implemented");
        let outcome = resolve_model_change("anything", Err(&e));
        assert_eq!(outcome, ModelChangeOutcome::Proceed { warning: None });
    }

    #[test]
    fn global_quit_allowed_is_false_for_splash_screen() {
        assert!(!super::global_quit_allowed(Some("splash")));
    }

    #[test]
    fn global_quit_allowed_is_true_for_other_contexts() {
        assert!(super::global_quit_allowed(None));
        assert!(super::global_quit_allowed(Some("palette")));
    }

    #[test]
    fn resolve_network_error_proceeds_with_warning() {
        use crate::test_helpers::HOME_LOCK;
        let _lock = HOME_LOCK.lock().unwrap();
        rust_i18n::set_locale("en");

        let e = err(ErrorKind::Network, "HTTP 401: invalid_api_key");
        let outcome = resolve_model_change("gpt-x", Err(&e));
        match outcome {
            ModelChangeOutcome::Proceed { warning: Some(w) } => {
                assert!(w.contains("Could not verify"), "w: {w}");
                assert!(w.contains("`gpt-x`"), "w: {w}");
                assert!(w.contains("invalid_api_key"), "w: {w}");
            }
            other => panic!("expected Proceed with warning, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod palette_shortcut_tests {
    use super::should_route_home_keybinding;

    #[test]
    fn palette_shortcut_requires_empty_prompt() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let slash = KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE);
        assert!(should_route_home_keybinding(&slash, &[String::new()]));
        assert!(!should_route_home_keybinding(
            &slash,
            &[String::from("draft")]
        ));
        assert!(!should_route_home_keybinding(
            &slash,
            &[String::from(""), String::from("still editing")]
        ));
    }

    #[test]
    fn non_palette_keys_still_route_with_prompt_text() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let ctrl_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert!(should_route_home_keybinding(
            &ctrl_p,
            &[String::from("draft")]
        ));
    }
}

#[cfg(test)]
mod connect_provider_selector_tests {
    use super::*;
    use crate::app::{App, InputMode};
    use crate::providers::effective_providers;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;

    fn fresh_app() -> App {
        App::new(String::new(), PathBuf::from("."), "en".to_string())
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn connect_provider_selector_typing_backspace_and_escape_edit_query() {
        let mut app = fresh_app();
        app.open_provider_selector();

        let first = handle_provider_selector_key(&mut app, key(KeyCode::Char('o')), |_| Ok(None));
        let second = handle_provider_selector_key(&mut app, key(KeyCode::Char('p')), |_| Ok(None));
        let backspace =
            handle_provider_selector_key(&mut app, key(KeyCode::Backspace), |_| Ok(None));
        let clear = handle_provider_selector_key(&mut app, key(KeyCode::Esc), |_| Ok(None));
        let dismiss = handle_provider_selector_key(&mut app, key(KeyCode::Esc), |_| Ok(None));

        assert!(first.is_none());
        assert!(second.is_none());
        assert!(backspace.is_none());
        assert!(clear.is_none());
        assert!(dismiss.is_none());
        assert!(app.provider_query.is_empty());
        assert!(matches!(app.input_mode, InputMode::Editing));
    }

    #[test]
    fn connect_provider_selector_backspace_to_empty_restores_active_provider() {
        let mut app = fresh_app();
        app.open_provider_selector();
        app.active_provider_id = Some("gemini");
        app.set_provider_query("open");

        let _ = handle_provider_selector_key(&mut app, key(KeyCode::Backspace), |_| Ok(None));
        let _ = handle_provider_selector_key(&mut app, key(KeyCode::Backspace), |_| Ok(None));
        let _ = handle_provider_selector_key(&mut app, key(KeyCode::Backspace), |_| Ok(None));
        let _ = handle_provider_selector_key(&mut app, key(KeyCode::Backspace), |_| Ok(None));

        assert!(app.provider_query.is_empty());
        assert_eq!(app.selected_provider().map(|spec| spec.id), Some("gemini"));
    }

    #[test]
    fn connect_provider_selector_escape_clears_to_first_provider_without_active_match() {
        let mut app = fresh_app();
        app.open_provider_selector();
        app.active_provider_id = Some("missing-provider");
        app.provider_index = effective_providers().len().saturating_sub(1);
        app.set_provider_query("open");

        let _ = handle_provider_selector_key(&mut app, key(KeyCode::Esc), |_| Ok(None));

        assert!(app.provider_query.is_empty());
        assert_eq!(
            app.selected_provider().map(|spec| spec.id),
            effective_providers().first().map(|spec| spec.id),
        );
        assert!(matches!(app.input_mode, InputMode::SelectingProvider));
    }

    #[test]
    fn connect_provider_selector_enter_keyless_provider_skips_prompt_and_lookup() {
        let mut app = fresh_app();
        app.open_provider_selector();
        app.set_provider_query("local");
        let mut lookup_calls = 0usize;

        let connect = handle_provider_selector_key(
            &mut app,
            key(KeyCode::Enter),
            |_| -> Result<Option<String>, String> {
                lookup_calls += 1;
                Ok(Some("should-not-be-used".into()))
            },
        );

        assert_eq!(lookup_calls, 0);
        assert!(matches!(app.input_mode, InputMode::Editing));
        assert_eq!(
            connect,
            Some(PendingProviderConnect {
                spec: effective_providers()
                    .into_iter()
                    .find(|spec| spec.id == "local")
                    .expect("local provider should exist"),
                api_key: String::new(),
            })
        );
    }

    #[test]
    fn connect_provider_selector_enter_keyed_provider_with_stored_key_opens_modal() {
        let mut app = fresh_app();
        app.open_provider_selector();
        app.set_provider_query("open");
        let mut lookup_calls = 0usize;

        let connect = handle_provider_selector_key(
            &mut app,
            key(KeyCode::Enter),
            |_| -> Result<Option<String>, String> {
                lookup_calls += 1;
                Ok(Some("stored-key".into()))
            },
        );

        assert_eq!(lookup_calls, 1);
        assert!(
            connect.is_none(),
            "a stored key must open the modal, not connect immediately"
        );
        assert!(matches!(app.input_mode, InputMode::EnteringApiKey));
        assert_eq!(app.pending_provider.map(|spec| spec.id), Some("openai"));
    }

    #[test]
    fn connect_provider_selector_enter_keyed_provider_falls_back_to_prompt_without_key() {
        let mut app = fresh_app();
        app.open_provider_selector();
        app.set_provider_query("open");

        let connect = handle_provider_selector_key(&mut app, key(KeyCode::Enter), |_| Ok(None));

        assert!(connect.is_none());
        assert!(matches!(app.input_mode, InputMode::EnteringApiKey));
        assert_eq!(app.pending_provider.map(|spec| spec.id), Some("openai"));
    }

    #[test]
    fn connect_provider_selector_enter_on_no_match_does_nothing() {
        let mut app = fresh_app();
        app.open_provider_selector();
        app.set_provider_query("zzz");

        let connect = handle_provider_selector_key(&mut app, key(KeyCode::Enter), |_| Ok(None));

        assert!(connect.is_none());
        assert!(matches!(app.input_mode, InputMode::SelectingProvider));
        assert!(app.pending_provider.is_none());
    }
}

#[cfg(test)]
mod api_key_modal_submit_tests {
    use super::*;
    use crate::app::{App, InputMode};
    use crate::providers::effective_providers;
    use std::path::PathBuf;

    fn fresh_app() -> App {
        App::new(String::new(), PathBuf::from("."), "en".to_string())
    }

    fn spec_by_id(id: &str) -> &'static ProviderSpec {
        effective_providers()
            .into_iter()
            .find(|spec| spec.id == id)
            .unwrap_or_else(|| panic!("{id} provider should exist"))
    }

    #[test]
    fn empty_submit_with_stored_key_reuses_it() {
        let mut app = fresh_app();
        let spec = spec_by_id("openai");
        app.enter_api_key_for(spec, true);

        let action = handle_api_key_modal_submit(&mut app, |_| Ok(Some("stored-key".into())));

        match action {
            ApiKeySubmitAction::Connect {
                spec: got_spec,
                api_key,
            } => {
                assert_eq!(got_spec.id, "openai");
                assert_eq!(api_key, "stored-key");
            }
            other => panic!("expected Connect with the stored key, got {other:?}"),
        }
        assert!(app.pending_provider.is_none());
    }

    #[test]
    fn empty_submit_with_no_stored_key_stays_in_modal() {
        let mut app = fresh_app();
        let spec = spec_by_id("openai");
        app.enter_api_key_for(spec, false);

        let action = handle_api_key_modal_submit(&mut app, |_| Ok(None));

        assert!(matches!(action, ApiKeySubmitAction::NoStoredKey));
        assert!(matches!(app.input_mode, InputMode::EnteringApiKey));
        assert!(app.pending_provider.is_some());
    }

    #[test]
    fn typed_key_replaces_stored_key() {
        let mut app = fresh_app();
        let spec = spec_by_id("openai");
        app.enter_api_key_for(spec, true);
        for c in "new-typed-key".chars() {
            app.api_key_textarea.input(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            ));
        }

        let action = handle_api_key_modal_submit(&mut app, |_| {
            panic!("load_creds must not be consulted when the user typed a key")
        });

        match action {
            ApiKeySubmitAction::Connect {
                spec: got_spec,
                api_key,
            } => {
                assert_eq!(got_spec.id, "openai");
                assert_eq!(api_key, "new-typed-key");
            }
            other => panic!("expected Connect with the typed key, got {other:?}"),
        }
        assert!(matches!(app.input_mode, InputMode::Editing));
    }

    #[test]
    fn no_modal_open_is_idle() {
        let mut app = fresh_app();
        let action = handle_api_key_modal_submit(&mut app, |_| Ok(None));
        assert!(matches!(action, ApiKeySubmitAction::Idle));
    }
}

/// Regression tests for issue #81 (reopened): the DeepSeek `/connect` picker
/// path could silently drop a rejected — or, on the very next attempt,
/// perfectly valid — credential whenever no host existed yet, because
/// `apply_pending_pool_add` bailed out before it ever re-validated the
/// stored key. See that function's doc comment for the full story.
#[cfg(test)]
mod apply_pending_pool_add_hostless_tests {
    use super::*;
    use crate::app::{Entry, PendingPoolAdd};
    use crate::plugin::builtin::provider_common::test_support::use_mock_keyring;
    use crate::test_helpers::{HOME_LOCK, HomeGuard};

    /// Build a host-less `App` (mirrors a fresh session, or one where the
    /// startup auto-connect found no usable credentials) via the same
    /// bootstrap path `main` uses, so the plugin runtime and `App` state
    /// are wired exactly like production.
    async fn hostless_app() -> (App, HostSlot, std::path::PathBuf, ToolBins) {
        build_app_with_host(
            HostBoot {
                host: None,
                header_model: "(disconnected)".into(),
                provider_id: None,
                startup_notes: Vec::new(),
                mcp_manager_seed: McpManagerSeed::default(),
                startup_verbose: false,
            },
            std::env::temp_dir(),
            ToolBins::default(),
        )
        .await
        .expect("build_app_with_host must succeed with no host")
    }

    fn notes(app: &App) -> Vec<String> {
        app.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Note(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    /// Before the fix: `Effect::RegisterProvider`'s silent-connect path
    /// (picked DeepSeek from the picker with no `--rekey`, stored key
    /// vanished/never validated) queued a `pending_pool_add`, and because no
    /// host existed yet `apply_pending_pool_add` returned immediately after
    /// a `tracing::warn!` — no note, no host, nothing the user could act on.
    /// A later message then failed with the generic, unhelpful "Not
    /// connected" — exactly issue #81's report. After the fix, the
    /// credential is still re-validated (here: found missing) and the
    /// outcome is always surfaced to the user, host or no host.
    // Test-only HOME/keyring serialization intentionally spans the awaits
    // below so concurrent tests can't race on the process-wide HOME
    // override or the shared mock keyring.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    #[serial_test::serial]
    async fn deepseek_reconnect_with_no_host_reports_missing_credential_instead_of_silently_dropping_it()
     {
        let _lock = HOME_LOCK.lock().unwrap();
        let _home = HomeGuard::new();
        use_mock_keyring();
        rust_i18n::set_locale("en");

        // No stored DeepSeek key and no env fallback: try_build_registration
        // must resolve to `Unavailable`, the same shape a rejected-then
        // never-fixed key eventually collapses to once the caller clears it.
        let _ = keyring::Entry::new("otto", "deepseek").map(|e| e.delete_credential());
        // SAFETY: HOME_LOCK held for the test's lifetime; no other test
        // reads/writes DEEPSEEK_API_KEY concurrently.
        unsafe { std::env::remove_var("DEEPSEEK_API_KEY") };

        let (mut app, host_slot, project_root, tool_bins) = hostless_app().await;
        assert!(
            current_host(&host_slot).await.is_none(),
            "test setup: session must start host-less"
        );

        app.pending_pool_add = Some(PendingPoolAdd {
            id: otto_plugin::ProviderId::new("deepseek").expect("valid id"),
            display_name: "DeepSeek".into(),
        });

        apply_pending_pool_add(&mut app, &host_slot, &project_root, &tool_bins, false).await;

        assert!(
            current_host(&host_slot).await.is_none(),
            "no usable credential must not fabricate a host"
        );
        let joined = notes(&app).join("\n");
        assert!(
            joined.contains("deepseek"),
            "the missing-credential outcome must be surfaced as a note \
             even with no host yet (previously silent); notes were: {joined}"
        );
    }

    /// Same host-less setup, but the pending id doesn't match any known
    /// provider — must not panic and must not fabricate a host either.
    // See the allow on the test above: HOME_LOCK intentionally spans the
    // awaits below.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    #[serial_test::serial]
    async fn unknown_provider_with_no_host_is_a_no_op() {
        let _lock = HOME_LOCK.lock().unwrap();
        let _home = HomeGuard::new();
        rust_i18n::set_locale("en");

        let (mut app, host_slot, project_root, tool_bins) = hostless_app().await;

        app.pending_pool_add = Some(PendingPoolAdd {
            id: otto_plugin::ProviderId::new("not-a-real-provider").expect("valid id"),
            display_name: "Not A Real Provider".into(),
        });

        apply_pending_pool_add(&mut app, &host_slot, &project_root, &tool_bins, false).await;

        assert!(current_host(&host_slot).await.is_none());
    }
}

#[cfg(test)]
mod connect_rejected_note_key_tests {
    use super::connect_rejected_note_key;
    use otto_protocol::ErrorKind;

    #[test]
    fn keyed_authentication_rejection_uses_rekey_note() {
        assert_eq!(
            connect_rejected_note_key(ErrorKind::Authentication, true),
            "notes.connect-rejected-keyed"
        );
    }

    #[test]
    fn keyed_non_authentication_rejection_uses_generic_note() {
        assert_eq!(
            connect_rejected_note_key(ErrorKind::RateLimited, true),
            "notes.connect-failed"
        );
    }
}

#[cfg(test)]
mod mcp_bootstrap_tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn resolve_configured_mcp_servers_builds_seed_and_endpoint_for_valid_stdio_entry() {
        let config_file = crate::config_file::ConfigFile {
            startup: Default::default(),
            migration: Default::default(),
            mcp_servers: vec![crate::config_file::McpServerEntry::Stdio {
                name: "fixture".into(),
                command: "/bin/echo".into(),
                args: vec!["hello".into()],
                env: HashMap::from([("TOKEN".into(), "literal".into())]),
            }],
            ..Default::default()
        };
        let mut notes = Vec::new();
        let (endpoints, seed) = resolve_configured_mcp_servers(&config_file, &mut notes).await;

        assert!(notes.is_empty());
        assert_eq!(seed.configured.len(), 1);
        assert!(seed.skip_notes.is_empty());
        assert_eq!(seed.configured[0].name, "fixture");
        assert_eq!(seed.configured[0].transport, "stdio");
        assert_eq!(seed.configured[0].target, "/bin/echo");
        assert_eq!(seed.configured[0].auth, McpServerAuthSummary::None);
        assert_eq!(endpoints.len(), 1);
        match &endpoints[0] {
            ToolEndpoint::Stdio {
                name,
                command,
                args,
                env,
            } => {
                assert_eq!(name, "fixture");
                assert_eq!(*command, PathBuf::from("/bin/echo"));
                assert_eq!(*args, vec!["hello".to_string()]);
                assert_eq!(env.get("TOKEN").map(String::as_str), Some("literal"));
            }
            other => panic!("expected stdio endpoint, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_configured_mcp_servers_records_skip_note_for_missing_secret() {
        let config_file = crate::config_file::ConfigFile {
            startup: Default::default(),
            migration: Default::default(),
            mcp_servers: vec![crate::config_file::McpServerEntry::Http {
                name: "remote".into(),
                url: "https://example.test/mcp".into(),
                auth: crate::config_file::McpAuthMode::Bearer,
            }],
            ..Default::default()
        };
        let mut notes = Vec::new();
        let (endpoints, seed) = resolve_configured_mcp_servers(&config_file, &mut notes).await;

        assert!(endpoints.is_empty());
        assert_eq!(seed.configured.len(), 1);
        assert_eq!(seed.configured[0].name, "remote");
        assert_eq!(seed.configured[0].transport, "http");
        assert_eq!(seed.configured[0].target, "https://example.test/mcp");
        assert_eq!(seed.configured[0].auth, McpServerAuthSummary::Bearer);
        assert_eq!(seed.skip_notes.len(), 1);
        assert_eq!(seed.skip_notes[0].0, "remote");
        assert!(seed.skip_notes[0].1.contains("missing keyring secret"));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("mcp server `remote` skipped"));
    }

    #[tokio::test]
    async fn resolve_configured_mcp_servers_records_skip_note_for_missing_oauth_state() {
        let config_file = crate::config_file::ConfigFile {
            startup: Default::default(),
            migration: Default::default(),
            mcp_servers: vec![crate::config_file::McpServerEntry::Http {
                name: "remote-oauth-missing".into(),
                url: "https://example.test/mcp".into(),
                auth: crate::config_file::McpAuthMode::Oauth,
            }],
            ..Default::default()
        };
        let mut notes = Vec::new();
        let (endpoints, seed) = resolve_configured_mcp_servers(&config_file, &mut notes).await;

        assert!(endpoints.is_empty());
        assert_eq!(seed.configured.len(), 1);
        assert_eq!(seed.configured[0].auth, McpServerAuthSummary::Oauth);
        assert_eq!(seed.skip_notes.len(), 1);
        assert_eq!(seed.skip_notes[0].0, "remote-oauth-missing");
        assert!(seed.skip_notes[0].1.contains("open /mcp to authorize"));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("mcp server `remote-oauth-missing` skipped"));
    }

    #[test]
    fn disconnected_mcp_manager_seed_summarizes_config_with_local_skip_notes() {
        let config_file = crate::config_file::ConfigFile {
            startup: Default::default(),
            migration: Default::default(),
            mcp_servers: vec![
                crate::config_file::McpServerEntry::Stdio {
                    name: "local".into(),
                    command: "/bin/echo".into(),
                    args: vec![],
                    env: HashMap::new(),
                },
                crate::config_file::McpServerEntry::Http {
                    name: "remote".into(),
                    url: "https://example.test/mcp".into(),
                    auth: crate::config_file::McpAuthMode::Oauth,
                },
            ],
            ..Default::default()
        };

        let seed = build_disconnected_mcp_manager_seed(&config_file);

        assert_eq!(seed.configured.len(), 2);
        assert_eq!(seed.configured[0].name, "local");
        assert_eq!(seed.configured[0].transport, "stdio");
        assert_eq!(seed.configured[0].auth, McpServerAuthSummary::None);
        assert_eq!(seed.configured[1].name, "remote");
        assert_eq!(seed.configured[1].transport, "http");
        assert_eq!(seed.configured[1].auth, McpServerAuthSummary::Oauth);
        assert_eq!(seed.skip_notes.len(), 1);
        assert_eq!(seed.skip_notes[0].0, "remote");
        assert!(seed.skip_notes[0].1.contains("open /mcp to authorize"));
    }
}

#[cfg(test)]
mod render_routing_show_tests {
    //! Tests for `render_routing_show` — the pure-over-snapshot
    //! renderer that pushes `/route show` output onto an `App`. Each
    //! case exercises a different branch (empty rules, active rule,
    //! skipped-because-disconnected rule, last-decision badge) and
    //! asserts on the number + content of the `Entry::Note` lines
    //! that landed on `app.entries`. Note assertions search for
    //! load-bearing substrings (rule names, "skipped", etc.) so the
    //! tests survive minor i18n wording tweaks.
    use super::*;
    use crate::app::{App, Entry};
    use async_trait::async_trait;
    use otto_host::{DefaultPick, RoutingRule, RoutingRules, RuleMatch};
    use otto_mcp::ProviderClient;
    use otto_protocol::{
        CompleteRequest, CompleteResponse, ListModelsResponse, ProviderError, ProviderId,
        StreamEvent,
    };
    use std::path::PathBuf;
    use tokio::sync::mpsc;

    /// Stub `ProviderClient` we never call — only used to satisfy the
    /// `registered_providers` map shape so `connected_provider_ids`
    /// returns the keys we just inserted.
    struct StubClient;
    #[async_trait]
    impl ProviderClient for StubClient {
        async fn complete(
            &self,
            _: CompleteRequest,
            _: Option<mpsc::Sender<StreamEvent>>,
        ) -> Result<CompleteResponse, ProviderError> {
            unreachable!("stub client never invoked in render_routing_show tests")
        }
        async fn list_models(&self) -> Result<ListModelsResponse, ProviderError> {
            unreachable!("stub client never invoked in render_routing_show tests")
        }
    }

    fn build_app() -> App {
        App::new("test-model".into(), PathBuf::from("/tmp"), "en".to_string())
    }

    fn register(app: &mut App, id: &str) {
        app.registered_providers
            .insert(id.to_string(), Box::new(StubClient));
    }

    fn collect_notes(app: &App) -> Vec<String> {
        app.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Note(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    fn rule_with_keyword(name: &str, provider: &str, model: &str, keyword: &str) -> RoutingRule {
        // `RuleMatch` is `#[non_exhaustive]` outside its defining crate;
        // construct with `Default::default()` and mutate the one field
        // we care about.
        let mut match_ = RuleMatch::default();
        match_.keywords = vec![keyword.into()];
        RoutingRule {
            name: name.into(),
            match_,
            use_: DefaultPick::new(ProviderId::new(provider).unwrap(), model).unwrap(),
        }
    }

    fn anthropic_rule(name: &str) -> RoutingRule {
        rule_with_keyword(name, "anthropic", "claude-opus-4-7", "x")
    }

    fn gemini_rule(name: &str) -> RoutingRule {
        rule_with_keyword(name, "gemini", "gemini-2.0-flash", "y")
    }

    #[test]
    fn empty_rules_prints_no_rules_then_default_then_last() {
        let mut app = build_app();
        render_routing_show(&mut app, &RoutingRules::empty());
        let notes = collect_notes(&app);
        // No rules → 1 line. No default → 1 line. No last decision → 1 line.
        assert_eq!(notes.len(), 3, "notes were: {notes:?}");
        assert!(
            notes[0].to_lowercase().contains("no routing rules"),
            "first note should be the no-rules line, got: {}",
            notes[0]
        );
    }

    #[test]
    fn one_active_rule_prints_rule_line_without_skipped_marker() {
        let mut app = build_app();
        register(&mut app, "anthropic");

        let rules = RoutingRules {
            default: None,
            heuristics: false,
            rules: vec![anthropic_rule("my-rule")],
        };
        render_routing_show(&mut app, &rules);
        let notes = collect_notes(&app);
        // header + 1 rule + no-default + no-last = 4 notes.
        assert_eq!(notes.len(), 4, "notes were: {notes:?}");
        let rule_line = &notes[1];
        assert!(
            rule_line.contains("my-rule") && rule_line.contains("anthropic"),
            "rule line missing name/provider: {rule_line}"
        );
        assert!(
            !rule_line.to_lowercase().contains("skipped"),
            "active rule must not carry the skipped marker: {rule_line}"
        );
    }

    #[test]
    fn one_skipped_rule_prints_skipped_marker() {
        let mut app = build_app();
        // Only anthropic registered; rule targets gemini → must be skipped.
        register(&mut app, "anthropic");

        let rules = RoutingRules {
            default: None,
            heuristics: false,
            rules: vec![gemini_rule("for-gemini")],
        };
        render_routing_show(&mut app, &rules);
        let notes = collect_notes(&app);
        assert_eq!(notes.len(), 4, "notes were: {notes:?}");
        let rule_line = &notes[1];
        assert!(
            rule_line.to_lowercase().contains("skipped"),
            "disconnected-target rule must carry the skipped marker: {rule_line}"
        );
    }

    #[test]
    fn last_decision_present_renders_it() {
        let mut app = build_app();
        app.entries.push(Entry::RouteBadge(
            "anthropic/claude-opus-4-7 — Rule(my-rule)".into(),
        ));

        render_routing_show(&mut app, &RoutingRules::empty());
        let notes = collect_notes(&app);
        // No rules + no default + last-decision (parsed from badge) = 3.
        assert_eq!(notes.len(), 3, "notes were: {notes:?}");
        let last = notes.last().expect("last present");
        assert!(
            last.contains("anthropic")
                && last.contains("claude-opus-4-7")
                && last.contains("Rule(my-rule)"),
            "last-decision note should include parsed badge fields: {last}"
        );
    }

    #[test]
    fn heuristic_active_line_shown_when_heuristics_true() {
        // rust_i18n::set_locale is process-global; serialize via
        // HOME_LOCK so a parallel test changing the locale can't
        // poison the English substring assertions below.
        let _g = crate::test_helpers::HOME_LOCK.lock().expect("home lock");
        rust_i18n::set_locale("en");

        let mut app = build_app();
        let rules = RoutingRules {
            default: None,
            heuristics: true,
            rules: vec![],
        };
        render_routing_show(&mut app, &rules);
        let notes = collect_notes(&app);
        let saw_active = notes
            .iter()
            .any(|n| n.to_lowercase().contains("heuristics: enabled"));
        assert!(
            saw_active,
            "expected an active-heuristics line; got {notes:?}"
        );
        // The "future release" placeholder line must NOT appear when
        // heuristics=true; that string belongs to the no-longer-emitted
        // routing.show-heuristics-pending key.
        assert!(
            !notes.iter().any(|n| n.contains("future release")),
            "'future release' placeholder must not be emitted when heuristics is on; got {notes:?}"
        );
    }

    #[test]
    fn heuristic_line_omitted_when_heuristics_false() {
        let _g = crate::test_helpers::HOME_LOCK.lock().expect("home lock");
        rust_i18n::set_locale("en");

        let mut app = build_app();
        let rules = RoutingRules {
            default: None,
            heuristics: false,
            rules: vec![],
        };
        render_routing_show(&mut app, &rules);
        let notes = collect_notes(&app);
        let saw_any_heuristics = notes
            .iter()
            .any(|n| n.to_lowercase().contains("heuristics"));
        assert!(
            !saw_any_heuristics,
            "no heuristic line should be emitted when heuristics is off; got {notes:?}"
        );
    }
}

#[cfg(test)]
mod resolve_initial_model_for_tests {
    //! Pins the four precedence layers of `resolve_initial_model_for`:
    //! `OTTO_MODEL` env > `~/.otto/models.toml` >
    //! `routing.toml#default` > `spec.default_model`. All four tests
    //! use the workspace-wide `HOME_LOCK` guard + `HomeGuard` to
    //! redirect `$HOME` to a fresh tempdir and serialise against every
    //! other `$HOME`-mutating test. `OTTO_MODEL` is set/unset
    //! inside the same critical section so the env-var precedence
    //! tests can't leak into a sibling test.
    use super::*;
    use crate::providers::ProviderSpec;
    use crate::test_helpers::{HOME_LOCK, HomeGuard};

    /// `ProviderSpec` we synthesise per-test. Stays inside the module
    /// so any future field additions only affect the test scaffolding.
    fn anthropic_spec() -> ProviderSpec {
        ProviderSpec {
            id: "anthropic",
            display_name: "Anthropic (test)",
            api_key_env: "ANTHROPIC_API_KEY",
            default_model: "claude-haiku-4-5",
            api_key_required: true,
        }
    }

    /// Write `body` to `~/.otto/<filename>` under the current
    /// `HomeGuard`'s tempdir.
    fn write_under_home(filename: &str, body: &str) {
        let home = std::env::var_os("HOME").expect("HOME set by HomeGuard");
        let dir = std::path::PathBuf::from(home).join(".otto");
        std::fs::create_dir_all(&dir).expect("create .otto dir");
        std::fs::write(dir.join(filename), body).expect("write file");
    }

    /// Clear `OTTO_MODEL` so no ambient env pollutes the test.
    /// Safe because we hold `HOME_LOCK` for the test lifetime.
    fn clear_env() {
        // SAFETY: HOME_LOCK is held; no other test mutates env right now.
        unsafe { std::env::remove_var("OTTO_MODEL") };
    }

    #[test]
    fn env_var_wins_over_everything() {
        let _lock = HOME_LOCK.lock().unwrap();
        let _home = HomeGuard::new();
        clear_env();
        // Plant a competing models.toml + routing.toml; env must still win.
        write_under_home(
            "models.toml",
            r#"schema_version = 1
[providers]
anthropic = "from-models-toml"
"#,
        );
        write_under_home("routing.toml", r#"default = "anthropic/from-routing-toml""#);
        // SAFETY: HOME_LOCK held.
        unsafe { std::env::set_var("OTTO_MODEL", "from-env") };

        let got = resolve_initial_model_for(&anthropic_spec());
        // Reset env BEFORE asserting so a failure doesn't pollute siblings.
        clear_env();
        assert_eq!(got, "from-env");
    }

    #[test]
    fn models_toml_wins_over_routing_toml_and_spec() {
        let _lock = HOME_LOCK.lock().unwrap();
        let _home = HomeGuard::new();
        clear_env();
        write_under_home(
            "models.toml",
            r#"schema_version = 1
[providers]
anthropic = "from-models-toml"
"#,
        );
        write_under_home("routing.toml", r#"default = "anthropic/from-routing-toml""#);

        let got = resolve_initial_model_for(&anthropic_spec());
        assert_eq!(got, "from-models-toml");
    }

    #[test]
    fn routing_toml_default_wins_over_spec_when_no_models_toml() {
        let _lock = HOME_LOCK.lock().unwrap();
        let _home = HomeGuard::new();
        clear_env();
        write_under_home("routing.toml", r#"default = "anthropic/from-routing-toml""#);

        let got = resolve_initial_model_for(&anthropic_spec());
        assert_eq!(got, "from-routing-toml");
    }

    #[test]
    fn falls_back_to_spec_default_when_all_empty() {
        let _lock = HOME_LOCK.lock().unwrap();
        let _home = HomeGuard::new();
        clear_env();
        // No models.toml, no routing.toml, no env — spec wins.
        let got = resolve_initial_model_for(&anthropic_spec());
        assert_eq!(got, "claude-haiku-4-5");
    }

    fn caps_for(models: &[&str], default: &str) -> otto_host::ProviderCapabilities {
        use otto_host::capabilities::{CostTier, ModelCapabilities, ProviderCapabilities};
        ProviderCapabilities::new(
            models
                .iter()
                .map(|m| ModelCapabilities {
                    id: (*m).into(),
                    display_name: (*m).into(),
                    supports_vision: false,
                    supports_audio: false,
                    context_window: 0,
                    cost_tier: CostTier::Standard,
                })
                .collect(),
            default.into(),
        )
        .expect("valid caps")
    }

    #[test]
    fn routing_toml_default_falls_back_when_model_not_in_catalog() {
        // Reproduces the rollup-blocker: user had
        // `default = "anthropic/from-routing-toml"` in routing.toml from
        // dogfood testing. Pre-fix the resolver returned that string,
        // Anthropic returned ModelNotFound on the first turn. Post-fix
        // the resolver falls back to spec.default_model and returns a
        // warning naming the offending entry.
        let _lock = HOME_LOCK.lock().unwrap();
        let _home = HomeGuard::new();
        clear_env();
        write_under_home("routing.toml", r#"default = "anthropic/from-routing-toml""#);

        let caps = caps_for(&["claude-haiku-4-5", "claude-opus-4-7"], "claude-haiku-4-5");
        let (model, warning) = resolve_initial_model_for_with_caps(&anthropic_spec(), Some(&caps));
        assert_eq!(
            model, "claude-haiku-4-5",
            "should fall back to spec default"
        );
        let w = warning.expect("warning must be surfaced");
        assert!(
            w.contains("from-routing-toml") && w.contains("/route reload"),
            "warning must name the offending value + tell the user how to fix it; got: {w}"
        );
    }

    #[test]
    fn routing_toml_default_passes_through_when_model_is_in_catalog() {
        // The happy path: routing.toml#default names a real model on the
        // provider. Resolver returns it as-is with no warning.
        let _lock = HOME_LOCK.lock().unwrap();
        let _home = HomeGuard::new();
        clear_env();
        write_under_home("routing.toml", r#"default = "anthropic/claude-opus-4-7""#);

        let caps = caps_for(&["claude-haiku-4-5", "claude-opus-4-7"], "claude-haiku-4-5");
        let (model, warning) = resolve_initial_model_for_with_caps(&anthropic_spec(), Some(&caps));
        assert_eq!(model, "claude-opus-4-7");
        assert!(warning.is_none(), "no warning expected; got {warning:?}");
    }

    #[test]
    fn caps_none_preserves_pre_fix_passthrough_behavior() {
        // When `caps` is None (e.g. early-startup paths that don't have a
        // ProviderRegistration yet), the resolver must not be stricter
        // than before — it returns whatever routing.toml says.
        let _lock = HOME_LOCK.lock().unwrap();
        let _home = HomeGuard::new();
        clear_env();
        write_under_home("routing.toml", r#"default = "anthropic/from-routing-toml""#);

        let (model, warning) = resolve_initial_model_for_with_caps(&anthropic_spec(), None);
        assert_eq!(model, "from-routing-toml");
        assert!(warning.is_none());
    }
}

#[cfg(test)]
mod canvas_key_tests {
    use super::*;
    use crate::canvas_input::{
        CANVAS_NEXT, CANVAS_PREV, adjacent_canvas, cycle_index, handle_focused_canvas_key,
    };
    use async_trait::async_trait;
    use otto_plugin::{
        ContentBlockId, ContentRenderer, FocusableElement, Frame, KeyCodePortable,
        KeyEventPortable, KeyMods, PixelFormat, PixelSize, Rect,
    };

    /// Build an empty `App` for canvas-key tests.
    fn build_app() -> App {
        App::new("test-model".into(), PathBuf::from("/tmp"), "en".to_string())
    }

    #[cfg(test)]
    mod startup_user_slash_command_tests {
        use super::*;
        use crate::test_helpers::{HOME_LOCK, HomeGuard};
        use otto_plugin::PluginId;

        // Test-only HOME serialization intentionally spans the startup awaits
        // so concurrent tests can't race on the process-wide HOME override.
        #[allow(clippy::await_holding_lock)]
        #[tokio::test(flavor = "current_thread")]
        async fn build_app_startup_skips_conflicting_user_exit_command() {
            let _lock = HOME_LOCK.lock().unwrap();
            let _home = HomeGuard::new();
            let project = tempfile::TempDir::new().unwrap();
            let dir = project.path().join(".otto/commands");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("exit.md"), "shadowed exit").unwrap();

            let (app, _host_slot, _project_root, _tool_bins) = build_app_with_host(
                HostBoot {
                    host: None,
                    header_model: "(disconnected)".into(),
                    provider_id: None,
                    startup_notes: Vec::new(),
                    mcp_manager_seed: McpManagerSeed::default(),
                    startup_verbose: false,
                },
                project.path().to_path_buf(),
                ToolBins::default(),
            )
            .await
            .expect("startup should not fail on a conflicting user /exit command");

            let indexes_handle = app.plugin_indexes.expect("plugin indexes installed");
            let indexes = indexes_handle.read().await;
            assert_eq!(
                indexes.slash.get("exit").map(PluginId::as_str),
                Some("internal:exit"),
                "startup must keep the built-in /exit command registered"
            );
            assert_eq!(
                indexes.slash.get("reload-commands").map(PluginId::as_str),
                Some("internal:user-slash-commands"),
                "startup must still register /reload-commands"
            );
        }
    }

    /// Empty host slot — Esc/Tab paths never touch it.
    fn empty_host_slot() -> HostSlot {
        Arc::new(RwLock::new(None))
    }

    fn key(code: KeyCodePortable) -> KeyEventPortable {
        KeyEventPortable {
            code,
            modifiers: KeyMods::default(),
        }
    }

    /// Stub renderer exposing `n` focusable elements and recording the
    /// last `set_focus` call so Tab traversal can be asserted.
    struct StubRenderer {
        id: ContentBlockId,
        n: usize,
        last_focus: Option<u32>,
    }

    #[async_trait]
    impl ContentRenderer for StubRenderer {
        fn id(&self) -> ContentBlockId {
            self.id
        }
        fn render(&mut self, _size: PixelSize) -> Frame {
            Frame {
                width: 1,
                height: 1,
                format: PixelFormat::Rgba8,
                bytes: vec![0, 0, 0, 0],
            }
        }
        fn focusable_elements(&self) -> Vec<FocusableElement> {
            (0..self.n)
                .map(|i| FocusableElement {
                    id: format!("el{i}"),
                    bounds: Rect {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1,
                    },
                })
                .collect()
        }
        fn set_focus(&mut self, index: Option<u32>) {
            self.last_focus = index;
        }
    }

    fn canvas(id: u32) -> Entry {
        Entry::Canvas {
            id: ContentBlockId(id),
            source: format!("<p>{id}</p>"),
            source_preview: None,
        }
    }

    #[test]
    fn adjacent_canvas_wraps_forward_and_back() {
        // 3 canvases interleaved with non-canvas entries.
        let entries = vec![
            Entry::Note("intro".into()),
            canvas(0),
            Entry::Note("mid".into()),
            canvas(5),
            canvas(9),
            Entry::Note("outro".into()),
        ];
        let next = |cur: u32| adjacent_canvas(&entries, ContentBlockId(cur), CANVAS_NEXT);
        let prev = |cur: u32| adjacent_canvas(&entries, ContentBlockId(cur), CANVAS_PREV);
        assert_eq!(next(0), Some(ContentBlockId(5)));
        assert_eq!(next(5), Some(ContentBlockId(9)));
        assert_eq!(next(9), Some(ContentBlockId(0)), "wraps to first");
        assert_eq!(prev(0), Some(ContentBlockId(9)), "wraps to last");
        assert_eq!(prev(9), Some(ContentBlockId(5)));
    }

    #[test]
    fn adjacent_canvas_single_returns_itself_and_empty_is_none() {
        let one = vec![canvas(3)];
        assert_eq!(
            adjacent_canvas(&one, ContentBlockId(3), CANVAS_NEXT),
            Some(ContentBlockId(3))
        );
        let none: Vec<Entry> = vec![Entry::Note("x".into())];
        assert_eq!(adjacent_canvas(&none, ContentBlockId(0), CANVAS_NEXT), None);
    }

    #[test]
    fn cycle_index_wraps_and_handles_none() {
        assert_eq!(cycle_index(None, 3, 1), Some(0), "None forward -> first");
        assert_eq!(cycle_index(None, 3, -1), Some(2), "None back -> last");
        assert_eq!(cycle_index(Some(0), 3, 1), Some(1));
        assert_eq!(cycle_index(Some(2), 3, 1), Some(0), "forward wraps");
        assert_eq!(cycle_index(Some(0), 3, -1), Some(2), "back wraps");
        assert_eq!(cycle_index(Some(1), 0, 1), None, "no elements -> None");
        assert_eq!(cycle_index(None, 0, 1), None);
    }

    #[tokio::test]
    async fn esc_returns_to_editing() {
        let mut app = build_app();
        let id = app.canvas_registry.allocate_id();
        app.entries.push(canvas(id.0));
        app.focus_canvas(id, None);
        assert!(app.is_canvas_focused(id));
        handle_focused_canvas_key(
            &mut app,
            &empty_host_slot(),
            id,
            None,
            key(KeyCodePortable::Esc),
        )
        .await;
        assert!(matches!(app.input_mode, InputMode::Editing));
    }

    #[tokio::test]
    async fn tab_advances_element_index_and_sets_renderer_focus() {
        let mut app = build_app();
        let id = app.canvas_registry.allocate_id();
        app.canvas_registry.insert(
            id,
            Box::new(StubRenderer {
                id,
                n: 3,
                last_focus: None,
            }),
        );
        app.entries.push(canvas(id.0));
        app.focus_canvas(id, None);

        // None -> 0
        handle_focused_canvas_key(
            &mut app,
            &empty_host_slot(),
            id,
            None,
            key(KeyCodePortable::Tab),
        )
        .await;
        assert!(matches!(
            app.input_mode,
            InputMode::Canvas {
                element_idx: Some(0),
                ..
            }
        ));

        // 0 -> 1
        handle_focused_canvas_key(
            &mut app,
            &empty_host_slot(),
            id,
            Some(0),
            key(KeyCodePortable::Tab),
        )
        .await;
        assert!(matches!(
            app.input_mode,
            InputMode::Canvas {
                element_idx: Some(1),
                ..
            }
        ));

        // BackTab 1 -> 0
        handle_focused_canvas_key(
            &mut app,
            &empty_host_slot(),
            id,
            Some(1),
            key(KeyCodePortable::BackTab),
        )
        .await;
        assert!(matches!(
            app.input_mode,
            InputMode::Canvas {
                element_idx: Some(0),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn ctrl_j_jumps_to_next_canvas() {
        let mut app = build_app();
        let a = app.canvas_registry.allocate_id();
        let b = app.canvas_registry.allocate_id();
        app.entries.push(canvas(a.0));
        app.entries.push(canvas(b.0));
        app.focus_canvas(a, None);

        let k = KeyEventPortable {
            code: KeyCodePortable::Char('j'),
            modifiers: KeyMods {
                ctrl: true,
                ..KeyMods::default()
            },
        };
        handle_focused_canvas_key(&mut app, &empty_host_slot(), a, None, k).await;
        assert!(app.is_canvas_focused(b), "Ctrl-J moves to the next canvas");
    }

    #[tokio::test]
    async fn ctrl_k_jumps_to_prev_canvas() {
        // Mirror of `ctrl_j_jumps_to_next_canvas` — Ctrl-K must move
        // focus to the previous canvas (wrapping). Starting focused on
        // `b`, Ctrl-K lands on `a`.
        let mut app = build_app();
        let a = app.canvas_registry.allocate_id();
        let b = app.canvas_registry.allocate_id();
        app.entries.push(canvas(a.0));
        app.entries.push(canvas(b.0));
        app.focus_canvas(b, None);

        let k = KeyEventPortable {
            code: KeyCodePortable::Char('k'),
            modifiers: KeyMods {
                ctrl: true,
                ..KeyMods::default()
            },
        };
        handle_focused_canvas_key(&mut app, &empty_host_slot(), b, None, k).await;
        assert!(
            app.is_canvas_focused(a),
            "Ctrl-K moves to the previous canvas",
        );
    }
}
