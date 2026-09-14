//! Regression anchor for "`/connect` always opens the API-key modal for a keyed provider,
//! even when a key is already stored."
//!
//! See `docs/superpowers/specs/2026-09-14-connect-stored-key-modal-design.md`.
//!
//! ## Background
//!
//! Before this fix, `/connect`'s picker dispatched `connect <id>` to each keyed provider
//! plugin, whose `handle_slash` read the keyring first and — if a stored key already produced a
//! working client — registered the provider immediately, skipping the API-key modal entirely.
//! The only escape hatch was `Alt+Enter` (which the picker translated into a `--rekey` arg), a
//! terminal-dependent modifier chord many terminals/multiplexers don't forward reliably. In
//! practice this left no discoverable, working way to change a stored key in the same session —
//! see `savvagent/otto#146` (reopened).
//!
//! As of this fix, every keyed provider's `handle_slash` always emits `Effect::PromptApiKey`
//! regardless of whether a key is already stored: the modal always opens, pre-filled with a
//! "press Enter to reuse, or paste a new key" placeholder when a stored key exists. Pressing
//! Enter on the empty field reuses the stored key (unchanged one-keystroke behavior); typing a
//! replacement saves and connects with the new one. The `Alt+Enter`/`--rekey` distinction is
//! retired — every path now behaves identically, so there is nothing left for it to do.
//!
//! ## Why this file is a stub
//!
//! `otto` is a binary-only crate: it has no `lib.rs`, so items under
//! `src/plugin/builtin/` are not reachable from `tests/` (integration tests
//! can only import `pub` items from a crate's library root). Adding a `lib.rs`
//! would require meaningful Cargo + module restructuring that would risk
//! breaking the TUI build mid-branch; that cost outweighs the cosmetic benefit
//! of moving the assertions here.
//!
//! ## Where the real tests live
//!
//! The scenarios that lock in this behavior are tested as `#[serial]` Tokio unit tests inside
//! the per-plugin `mod tests` blocks:
//!
//! | Test name | Location |
//! |---|---|
//! | `handle_slash_with_stored_key_opens_modal_for_confirm_or_replace` (anthropic) | `crates/otto/src/plugin/builtin/provider_anthropic/mod.rs` |
//! | `handle_slash_with_stored_key_opens_modal_for_confirm_or_replace` (gemini) | `crates/otto/src/plugin/builtin/provider_gemini/mod.rs` |
//! | `handle_slash_with_stored_key_opens_modal_for_confirm_or_replace` (openai) | `crates/otto/src/plugin/builtin/provider_openai/mod.rs` |
//! | `handle_slash_with_stored_key_opens_modal_for_confirm_or_replace` (grok) | `crates/otto/src/plugin/builtin/provider_grok/mod.rs` |
//! | `handle_slash_with_stored_key_opens_modal_for_confirm_or_replace` (deepseek) | `crates/otto/src/plugin/builtin/provider_deepseek/mod.rs` |
//! | `handle_slash_with_rekey_flag_opens_modal_even_when_client_exists` (all five, same locations) | kept as a regression guard that `--rekey` remains harmless |
//! | `alt_enter_routes_identically_to_plain_enter` | `crates/otto/src/plugin/builtin/connect/screen.rs` |
//! | `empty_submit_with_stored_key_reuses_it`, `typed_key_replaces_stored_key`, and siblings in `api_key_modal_submit_tests` | `crates/otto/src/main.rs` |
//!
//! Run them with:
//!
//! ```text
//! cargo test -p otto -- handle_slash_with_stored_key_opens_modal_for_confirm_or_replace
//! cargo test -p otto -- handle_slash_with_rekey_flag_opens_modal_even_when_client_exists
//! cargo test -p otto -- api_key_modal_submit_tests
//! ```
//!
//! `git grep handle_slash_with_stored_key_opens_modal_for_confirm_or_replace` will find all five
//! provider sites at once.
//!
//! ## If you are refactoring this path
//!
//! You are about to touch code guarded by the tests listed above. Before merging, verify that
//! **all five providers** still emit `PromptApiKey` (never `RegisterProvider`) when a keyring
//! entry is present at `connect <id>` time, that empty-submit still reuses the stored key, and
//! that a typed replacement still saves and connects with the new value.
