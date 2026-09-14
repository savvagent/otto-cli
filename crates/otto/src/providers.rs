//! Provider catalog.
//!
//! Each entry describes one supported LLM provider. The TUI links every
//! built-in provider crate as a library; nothing is spawned.
//!
//! Adding a new built-in provider is a one-entry change — implement
//! `ProviderHandler` in a new crate, expose a builder, and append to
//! [`PROVIDERS`].
//!
//! ## External (wasm) provider plugins
//!
//! Discovered wasm plugins that implement the `plugin-provider` world
//! contribute additional `ProviderSpec` entries at runtime via
//! [`install_external_providers`]. The combined catalog (built-ins first,
//! externals appended in discovery order) is the source of truth every
//! TUI surface should read; call [`effective_providers`] instead of
//! iterating [`PROVIDERS`] directly so external providers show up in
//! `/connect`, `/model`, and the splash banner alongside built-ins.
//!
//! `EXTERNAL_PROVIDERS` is a [`std::sync::OnceLock`] — set exactly once
//! by the TUI's bootstrap path (right after [`crate::plugin::register_builtins_with_external`]).
//! Mutating the catalog after startup (`/plugins reload <id>`) is a Task
//! 11+ follow-up; for v0.18.0 the user restarts the TUI to pick up a
//! re-trusted plugin.

use std::sync::OnceLock;

/// Static metadata for one provider.
///
/// Built-in entries below use string literals for every field; external
/// (wasm-discovered) entries leak `Box<str>` into `'static` storage at
/// startup so the `&'static str` shape is uniform across both
/// populations. Keeping `Copy` lets the rest of the TUI hold `Option<&'static ProviderSpec>`
/// fields without lifetime gymnastics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSpec {
    /// Stable identifier — keyring account name and `/connect` selector key.
    pub id: &'static str,
    /// Pretty name shown in the selector.
    pub display_name: &'static str,
    /// The env var the underlying SDK conventionally reads. Used only as a
    /// hint in the API-key prompt; we never actually read or set it.
    /// For keyless providers (see [`api_key_required`]) this is the URL
    /// override env var instead.
    pub api_key_env: &'static str,
    /// Default model id passed to the host when this provider connects.
    pub default_model: &'static str,
    /// When `false`, the `/connect` flow skips the API-key prompt and the
    /// keyring read/write entirely.
    pub api_key_required: bool,
}

/// Built-in providers the TUI offers in `/connect`. External wasm
/// providers extend this list at runtime via [`install_external_providers`];
/// call [`effective_providers`] to see the combined catalog.
pub const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "anthropic",
        display_name: "Anthropic (Claude)",
        api_key_env: "ANTHROPIC_API_KEY",
        default_model: "claude-haiku-4-5",
        api_key_required: true,
    },
    ProviderSpec {
        id: "gemini",
        display_name: "Google Gemini",
        api_key_env: "GEMINI_API_KEY",
        default_model: "gemini-2.5-flash",
        api_key_required: true,
    },
    ProviderSpec {
        id: "openai",
        display_name: "OpenAI",
        api_key_env: "OPENAI_API_KEY",
        default_model: "gpt-4o-mini",
        api_key_required: true,
    },
    ProviderSpec {
        id: "deepseek",
        display_name: "DeepSeek",
        api_key_env: "DEEPSEEK_API_KEY",
        default_model: "deepseek-v4-flash",
        api_key_required: true,
    },
    ProviderSpec {
        id: "grok",
        display_name: "xAI Grok",
        api_key_env: "XAI_API_KEY",
        default_model: "grok-4",
        api_key_required: true,
    },
    ProviderSpec {
        id: "local",
        display_name: "Ollama (local)",
        api_key_env: "OLLAMA_HOST",
        default_model: "llama3.2",
        api_key_required: false,
    },
];

/// Externally-discovered wasm provider specs. Populated once at startup
/// by [`install_external_providers`]; surfaces through
/// [`effective_providers`] alongside the built-in [`PROVIDERS`] slice.
///
/// `OnceLock<Vec<_>>` (not `RwLock`) because the v0.18.0 lifecycle is
/// strictly "set once at boot, read for the rest of the process". A
/// mutable catalog (for `/plugins reload`) is a Task 11+ extension that
/// would swap this for a `RwLock<Vec<_>>` without changing call-site
/// shapes — every reader already goes through [`effective_providers`].
static EXTERNAL_PROVIDERS: OnceLock<Vec<ProviderSpec>> = OnceLock::new();

/// Provider-count cutoff where the `/connect` modal should show its filter
/// row even before the user types. At five-or-fewer entries we keep the
/// existing uncluttered list-only modal; the sixth provider makes search
/// discoverability worth the extra row.
#[allow(dead_code)]
pub const PROVIDER_SELECTOR_DISCOVERABILITY_THRESHOLD: usize = 5;

/// Case-insensitive subsequence match against a provider's stable id or
/// display name. Empty queries match every provider so callers can always
/// derive their visible list from `effective_providers()` through one helper.
pub(crate) fn provider_matches_query(spec: &ProviderSpec, query: &str) -> bool {
    provider_label_matches_query(spec.id, spec.display_name, query)
}

/// Case-insensitive subsequence match against a provider id/display-name pair.
/// Shared by the legacy `SelectingProvider` modal and the plugin-owned
/// `connect.picker` screen so `/connect` stays consistent whichever surface
/// opens it.
pub(crate) fn provider_label_matches_query(id: &str, display_name: &str, query: &str) -> bool {
    query.is_empty()
        || matches_case_insensitive_subsequence(id, query)
        || matches_case_insensitive_subsequence(display_name, query)
}

fn matches_case_insensitive_subsequence(haystack: &str, needle: &str) -> bool {
    let mut needle = needle.chars().flat_map(char::to_lowercase);
    let mut current = needle.next();
    if current.is_none() {
        return true;
    }

    for hay in haystack.chars().flat_map(char::to_lowercase) {
        if Some(hay) == current {
            current = needle.next();
            if current.is_none() {
                return true;
            }
        }
    }

    false
}

/// Install discovered wasm provider specs into the runtime catalog.
///
/// Called once by the TUI bootstrap with the `[exports] provider-id`
/// taken from every successfully-loaded `plugin-provider` plugin. Each
/// `ProviderSpec`'s `id`/`display_name`/`api_key_env`/`default_model`
/// fields are `&'static str`; the caller is responsible for leaking the
/// underlying `String`s into 'static storage before constructing the
/// spec. The otto crate's bootstrap path does this with
/// `String::leak` once per plugin.
///
/// Subsequent calls (e.g. a second bootstrap path in tests) are silently
/// ignored — [`OnceLock::set`] returns the previously-stored value as
/// `Err`. Callers that need to verify install success can read it back
/// via [`effective_providers`].
pub fn install_external_providers(specs: Vec<ProviderSpec>) {
    // OnceLock::set returns Err if already set; ignore — we never need
    // to update after the first call.
    let _ = EXTERNAL_PROVIDERS.set(specs);
}

/// Built-in providers followed by any wasm-plugin providers installed at
/// startup. Returned in discovery order so the first match in
/// `iter().find(|s| s.id == …)` chains is deterministic.
///
/// Returns `&'static ProviderSpec` references rather than owned values
/// so callers can compare references and store handles indefinitely —
/// both the built-in slice and the externally-set `OnceLock` storage
/// live for the rest of the process.
pub fn effective_providers() -> Vec<&'static ProviderSpec> {
    let mut v: Vec<&'static ProviderSpec> = PROVIDERS.iter().collect();
    if let Some(ext) = EXTERNAL_PROVIDERS.get() {
        for s in ext {
            v.push(s);
        }
    }
    v
}

/// Render picker-based recovery guidance for a turn-time authentication
/// failure, or `None` if the routed provider is unknown or doesn't take an
/// API key (in which case there would be nothing useful to guide). The
/// rendered string instructs running `/connect` and picking the provider,
/// and entering (or reusing) a key in the modal that opens.
///
/// `routed_provider_id` should come from the per-turn `TurnEvent::RouteSelected`
/// capture, not `App::active_provider_id` — routing (`@`-override, modality
/// redirection, `routing.toml`) can select a different pool entry than the
/// active one for a given turn.
pub(crate) fn turn_auth_hint(
    routed_provider_id: Option<&otto_protocol::ProviderId>,
    provider_display_name: &str,
) -> Option<String> {
    let id = routed_provider_id?;
    let spec = effective_providers()
        .into_iter()
        .find(|spec| spec.id == id.as_str())?;
    if !spec.api_key_required {
        return None;
    }
    Some(
        rust_i18n::t!(
            "notes.turn-auth-failed-hint",
            id = spec.id,
            name = provider_display_name
        )
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `effective_providers` returns the built-in slice when no external
    /// providers have been installed. (We can't reset `OnceLock` between
    /// tests so this only holds before any other test in the same process
    /// calls `install_external_providers`. The current TUI lifecycle never
    /// calls it in tests — the only call is in the integration test
    /// crate, which gets its own process — so this is a stable assertion.)
    #[test]
    fn effective_providers_includes_builtins() {
        let eff = effective_providers();
        assert!(eff.len() >= PROVIDERS.len());
        // Built-in slice is contained as the prefix.
        for (i, spec) in PROVIDERS.iter().enumerate() {
            assert_eq!(eff[i].id, spec.id);
        }
    }

    #[test]
    fn provider_matches_query_checks_id_and_display_name() {
        let openai = PROVIDERS
            .iter()
            .find(|spec| spec.id == "openai")
            .expect("openai provider should exist");
        let anthropic = PROVIDERS
            .iter()
            .find(|spec| spec.id == "anthropic")
            .expect("anthropic provider should exist");

        assert!(provider_matches_query(openai, "opn"));
        assert!(provider_matches_query(anthropic, "cld"));
        assert!(!provider_matches_query(openai, "zzz"));
    }

    #[test]
    fn turn_auth_hint_only_for_known_keyed_providers() {
        rust_i18n::set_locale("en");

        let gemini_id = otto_protocol::ProviderId::new("gemini").expect("valid provider id");
        let local_id = otto_protocol::ProviderId::new("local").expect("valid provider id");

        let gemini_hint = turn_auth_hint(Some(&gemini_id), "Gemini");
        assert!(gemini_hint.is_some());
        let text = gemini_hint.as_deref().unwrap();
        assert!(
            text.contains("/connect"),
            "hint should mention /connect: {text}"
        );
        assert!(
            !text.contains("Alt+Enter"),
            "hint should no longer mention Alt+Enter (every stored-key /connect \
             selection opens the modal now): {text}"
        );
        assert!(
            text.contains("Gemini"),
            "hint should mention Gemini: {text}"
        );
        assert!(
            !text.contains("--rekey"),
            "hint must not mention --rekey: {text}"
        );
        assert!(
            !text.contains("/connect gemini"),
            "hint must not mention /connect gemini: {text}"
        );
        assert_eq!(turn_auth_hint(None, "Gemini"), None);
        assert_eq!(turn_auth_hint(Some(&local_id), "Local"), None);
    }
}
