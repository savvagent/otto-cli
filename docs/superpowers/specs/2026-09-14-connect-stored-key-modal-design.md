# `/connect` silently reconnects with a stored key instead of prompting — design

Date: 2026-09-14
Status: pending review
Source: savvagent/otto#146 (reopened)

## Problem

`savvagent/otto#146` was originally filed against a TUI freeze (raw MCP/`tracing` log output
rendered into the transcript pane on a mid-session DeepSeek `/connect`) and was fixed by #163
(`spawn_tool_transport`, shipped in v0.30.5). The issue owner (`robhicks`) reopened it on
2026-09-14 after confirming the freeze is gone but reporting a second, distinct defect on the exact
same repro path: **selecting a provider that already has a stored API key never opens the API-key
entry modal at all** — `/connect` → pick DeepSeek → the picker just silently reconnects with
whatever key is already in the OS keyring, with no way to type a replacement key in that session.

This is a pre-existing gap, not a regression from #163 or from #132/#81 — per the reopening
comment's own root-cause note, it has existed since PR #72 (the original connect-selector
fuzzy-search change). #146's freeze was simply masking it: every previous manual repro of "connect
DeepSeek" hit the freeze before the tester could notice the modal never opened.

### Root cause (confirmed by reading the current code, not inferred)

- `submit_selected_provider` (`crates/otto/src/main.rs:4419-4450`) is called on `Enter` from the
  `/connect` provider picker (`handle_provider_selector_key`, `main.rs:4491`). Its `Ok(Some(key))`
  arm — reached whenever `creds::load(spec.id)` finds a stored credential — pushes a
  `notes.using-stored-key` note and immediately returns `Some(PendingProviderConnect { spec, api_key:
  key })`, which the caller (`run_app`'s `InputMode::SelectingProvider` arm, `main.rs:4208-4221`)
  feeds straight into `perform_connect` — no modal, no prompt, no way to intervene.
- `Ok(None)` (no stored key) and `Err(_)` (keyring read failed) both correctly call
  `app.enter_api_key_for(spec, false)`, which opens the masked-input modal
  (`InputMode::EnteringApiKey`).
- `App::enter_api_key_for(spec, has_stored)` (`crates/otto/src/app.rs:1833-1849`) already has full
  support for the "confirm-or-replace" case: when `has_stored` is `true` it sets the modal's
  placeholder to `prompt.api-key.use-stored-or-paste-new` ("Press Enter to use stored key, or paste
  a new key"), and the placeholder-selection logic is covered by an existing unit test
  (`provider_selector_api_key_entry_helper_preserves_placeholder_behavior`,
  `crates/otto/src/app.rs:2718-2744`) — but **no production call site ever passes `true`**.
  `enter_api_key_for(spec, true)` is exercised only by that one direct-unit test; it is dead code on
  every real `/connect` path.
- The modal's own `Enter`-on-empty-input handling (`run_app`'s `InputMode::EnteringApiKey` arm,
  `main.rs:4223-4261`) already implements exactly the "one-keystroke reuse" behavior the fix needs:
  `app.take_pending_api_key()` returning `Some((spec, None))` (empty submit) falls back to
  `creds::load(spec.id)` and, if found, connects with the stored key
  (`main.rs:4236-4248`); if none is found, it stays in the modal and pushes `notes.api-key-empty`
  (`main.rs:4249-4253`). This path is unreachable today only because the modal never opens when a
  key is already stored — `submit_selected_provider` intercepts and shortcuts around it first.
- `README.md:122` documents `/connect` as "Silent when the keyring already has a stored key — the
  API-key modal only opens when a key is missing, or when pressed with `Alt+Enter` to re-key." No
  `Alt+Enter` (or any `KeyModifiers::ALT`) handling exists anywhere in `handle_provider_selector_key`
  or the `SelectingProvider`/`EnteringApiKey` arms of `run_app` — grepped and confirmed absent. This
  is stale/aspirational documentation describing a re-key mechanism that was never implemented, not
  a description of working behavior this fix needs to preserve. Per `CLAUDE.md`'s own precedence rule
  ("when [CLAUDE.md] disagrees with the current code... the code wins" — the same applies to
  README.md, which can equally lag an evolving codebase) and the Iron Law (the GitHub issue is the
  source of truth for this task), this spec corrects the documentation to describe the new, actual
  behavior rather than preserving a feature description nothing implements.

### Why this wasn't caught by review sooner

`connect_provider_selector_enter_keyed_provider_uses_stored_key_before_prompting`
(`crates/otto/src/main.rs:4809-4837`) is an existing, passing unit test that asserts *exactly* the
undesired behavior this spec fixes: that submitting a keyed provider with a stored credential
returns an immediate `PendingProviderConnect` rather than opening the modal. The test's own name
states the behavior as a feature ("uses stored key before prompting"). It was accurate to the code
at the time it was written and is not itself a bug — it correctly pinned down what the code did —
but it means the short-circuit was locked in by a green test suite rather than caught by it. This
spec's plan replaces that test with one asserting the corrected behavior (Task 1).

## Approach

Make `submit_selected_provider`'s three `load_creds` outcomes converge on the same shape:
always open the API-key modal via `app.enter_api_key_for(spec, has_stored)`, varying only the
`has_stored` flag and, on a keyring read error, the note pushed first. Concretely, in
`crates/otto/src/main.rs`:

```rust
match load_creds(spec) {
    Ok(Some(_)) => {
        // A credential is already stored — open the modal instead of
        // connecting immediately, so the user can press Enter to reuse it
        // or type a replacement. See savvagent/otto#146.
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
```

`submit_selected_provider` never returns `Some(PendingProviderConnect { .. })` for a keyed provider
anymore (only the `!spec.api_key_required` early return at the top of the function still does,
unchanged — a keyless provider like `local` has no key to confirm or replace, so it keeps
connecting immediately). The stored key's actual value is no longer read out of `load_creds`'s
`Ok(Some(key))` payload at this call site — only its presence is used to pick the placeholder — so
the binding is renamed `_` to make that explicit.

The `notes.using-stored-key` locale string (`crates/otto/locales/{en,es,hi,pt}.toml`) stops being
pushed at *selection* time (misleading — no connection happens yet) and instead moves to the moment
a stored key is actually reused: the empty-submit branch of the `EnteringApiKey` handling. To make
that branch (and the "type a replacement" branch next to it) independently unit-testable — mirroring
the existing `handle_provider_selector_key`/`submit_selected_provider` split, which was built
exactly for this kind of dependency-injected testability — extract the inline `KeyCode::Enter` match
arm currently in `run_app` (`main.rs:4225-4257`) into a new pure(ish) helper:

```rust
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
            _ => {
                app.push_note(rust_i18n::t!("notes.api-key-empty").to_string());
                ApiKeySubmitAction::NoStoredKey
            }
        },
        None => ApiKeySubmitAction::Idle,
    }
}
```

`run_app`'s `InputMode::EnteringApiKey => match key.code { KeyCode::Enter => ... }` arm becomes:

```rust
KeyCode::Enter => {
    match handle_api_key_modal_submit(app, |spec| {
        creds::load(spec.id).map_err(|e| format!("{e:#}"))
    }) {
        ApiKeySubmitAction::Connect { spec, api_key } => {
            perform_connect(spec, api_key, &host_slot, &project_root, &tool_bins, app).await;
        }
        ApiKeySubmitAction::NoStoredKey | ApiKeySubmitAction::Idle => {}
    }
}
```

This is a pure refactor of already-existing control flow (same branches, same order, same notes,
same `perform_connect` call) plus the one added `push_note` call that restores the
"using stored key" message at its corrected moment — no new async boundary, no new `.await` inside a
lock, no change to `perform_connect` itself.

`README.md`'s `/connect` row (line 122) is corrected to describe the shipped behavior instead of the
never-implemented `Alt+Enter` mechanism:

> `/connect` | Open the provider picker to add a provider to the connection pool. If the keyring
> already has a stored key for the selected provider, the API-key modal opens with a
> "press Enter to reuse, or paste a new key" placeholder — press Enter on the empty field to keep
> using the stored key, or type a replacement to save and connect with a new one. Multiple providers
> can be connected simultaneously; switch with `/use <provider>`.

## Scope

**In:**
- `crates/otto/src/main.rs` — `submit_selected_provider`'s `Ok(Some(_))` arm; the new
  `ApiKeySubmitAction` enum and `handle_api_key_modal_submit` helper; `run_app`'s
  `InputMode::EnteringApiKey`/`KeyCode::Enter` arm rewritten to call it; test updates (rename/rewrite
  the test that pinned the old short-circuit behavior; add tests for the new helper).
- `README.md` — correct the `/connect` row's description (line 122) to match the new (and actual)
  behavior; remove the stale `Alt+Enter` claim.
- `CHANGELOG.md` — a `Fixed` entry (added in the dedicated release PR per Non-Negotiable Rule 8 /
  Phase 4 step 12, not in this PR).

**Out:**
- Anything already fixed by #163 (`spawn_tool_transport`, the stderr-inherit leak) — confirmed still
  correct on current `main`, not re-touched here.
- `App::enter_api_key_for`, `App::take_pending_api_key`, `App::cancel_connect`
  (`crates/otto/src/app.rs`) — already correct and already tested; this fix only changes who calls
  `enter_api_key_for` with `has_stored = true`, not the function itself.
- `perform_connect`, `bootstrap_first_pool_host`, `apply_pending_pool_add` — unaffected; they still
  receive a `(spec, api_key)` pair exactly as before, from a different (correct) call path.
- Adding the `Alt+Enter` re-key mechanism README.md previously (inaccurately) described. Once every
  keyed-provider selection opens the modal unconditionally, there is nothing left for a separate
  re-key keybinding to do — the modal *is* the re-key path now. Implementing a keybinding for a
  behavior this fix makes redundant would be scope creep; the fix instead corrects the documentation
  to match reality.
- The host-swap `RwLock` discipline (`crates/otto/src/app.rs`/`tui.rs`) — this fix touches no lock
  acquisition, no `.await` under a guard, and no host-swap code at all.
- The provider transport split, `ToolRegistry` stdio plumbing, or the `ProgressDispatcher`
  forwarder-abort pattern — none of this is touched; the change is confined to the picker/modal input
  state machine in `crates/otto/src/main.rs`.

## Public-interface changes

**Additive/bug-fix, not breaking**, per Non-Negotiable Rule 6's own framing. `/connect` is a
documented slash command (`README.md`), and its *interactive behavior* changes: a keyed provider
selection with a stored credential now requires one more keystroke (Enter, or a typed replacement)
before connecting, instead of connecting immediately. This is corrective, not a removal or rename of
the command, an input-schema change, a wire-format change, or an on-disk format change — none of
Rule 6's breaking-change examples (renaming/removing a tool/field/slash command, changing a
`StreamEvent` variant, changing the transcript/keyring on-disk format) apply. This does mean a
scripted/headless caller that drives `/connect` by keystroke and previously reached a connected
state on a single Enter (picker selection *and* connection in one keystroke, because the old code
short-circuited straight into `perform_connect`) now needs a second Enter: the first now only opens
the modal, and a second Enter (submitted against the now-open `EnteringApiKey` textarea) is what
actually reuses the stored key and connects. An interactive human user is unaffected in practice —
pressing Enter twice across two prompts is the same gesture a keyless-provider or no-stored-key
`/connect` flow already required. No slash command is added, renamed, or removed; no `ProviderSpec`
field changes; no on-disk keyring/transcript format changes. Treated as a `Fixed` entry (PATCH), not
a breaking change requiring a MINOR bump — Rule 6's breaking-change taxonomy is about wire
formats/schemas/command renames, not the keystroke count of an interactive TUI flow.

## Assumptions

- **The issue's reopening comment is the current, authoritative AC for this task**, superseding the
  original issue body's freeze/log-rendering report, which #163 already fixed and which this spec
  does not revisit. The Iron Law names "the GitHub issue" as the source of truth without excluding
  its comments; the reopening comment is where the repo owner recorded the confirmed root cause and
  the updated acceptance criteria after re-testing v0.30.5.
- **`README.md`'s `Alt+Enter` claim is corrected, not implemented.** Building a real `Alt+Enter`
  keybinding was considered and rejected (see Scope/Out) — it would duplicate the modal's own
  confirm-or-replace flow that this fix makes universally reachable, for no behavioral gain.
- **The `notes.using-stored-key` locale string is repurposed (moved), not removed or added to.** All
  four locale files (`en`, `es`, `hi`, `pt`) already carry it; no translation work is needed, only a
  different call site.
- **Extracting `handle_api_key_modal_submit` is in scope, not gold-plating**, because the updated
  acceptance criteria explicitly ask for "a regression test covering... the path" and the inline
  `KeyCode::Enter` arm inside `run_app` is not unit-testable in isolation (it closes over
  `host_slot`/`project_root`/`tool_bins` and calls the async, I/O-performing `perform_connect`
  directly). The extraction mirrors this file's own established pattern
  (`handle_provider_selector_key` alongside `submit_selected_provider`) rather than inventing a new
  one.
- **No change to `perform_connect` itself.** It already persists whatever key it's given via
  `creds::save` before building the registration (`main.rs:2794-2802`), so "type a replacement and
  submit" already correctly overwrites the stored credential — that half of the updated AC needs no
  code change, only the fixed reachability that lets a user get to the modal at all.

## Goal & Success Criteria

Selecting a provider in `/connect` that already has a stored key always opens the API-key modal
(pre-filled placeholder advertising "press Enter to reuse, or paste a new key") instead of
connecting immediately; pressing Enter on the empty field reuses the stored key exactly as before;
typing a new key and submitting replaces the stored credential and connects with it.

- `submit_selected_provider` never returns `Some(PendingProviderConnect { .. })` for a keyed
  provider with a stored credential — it always opens the modal via `enter_api_key_for(spec, true)`.
- A new/updated unit test in `crates/otto/src/main.rs` proves: (a) selecting a keyed provider with a
  stored credential opens the modal (`InputMode::EnteringApiKey`, `pending_provider` set), not an
  immediate connect; (b) submitting the modal empty reuses the stored key
  (`ApiKeySubmitAction::Connect` with the stored value); (c) typing a replacement and submitting
  connects with the typed value, not the stored one.
- `README.md`'s `/connect` row no longer claims an `Alt+Enter` mechanism that doesn't exist.
- `cargo test -p otto` passes, including the new/updated tests.
- `cargo build --workspace --all-targets`, `cargo clippy --workspace --all-targets`, and
  `cargo fmt --all --check` are clean.

## Error Handling & Edge Cases

- **Keyring read error on selection (`Err(err)` arm).** Unchanged: still pushes
  `notes.keyring-error` and opens the modal with `has_stored = false` (there is no known-good stored
  value to offer reuse of, so the "use stored" placeholder would be misleading).
- **Keyring read error on empty-submit fallback.** Unchanged existing behavior: `creds::load`
  returning `Err` on the empty-submit path falls into the same `_` arm as `Ok(None)` (no stored key)
  — stays in the modal, pushes `notes.api-key-empty`. Not distinguishing the two cases here is
  pre-existing behavior, out of scope for this fix.
- **Keyless providers (`spec.api_key_required == false`, e.g. `local`).** Unaffected — the early
  return at the top of `submit_selected_provider` still connects immediately; there is no key to
  confirm or replace.
- **`Esc` inside the modal.** Unchanged: `app.cancel_connect()` still aborts back to `Editing` with
  no connection attempt, regardless of whether a stored key existed.

## Risks & Open Questions

- **None identified requiring escalation.** The fix is a small, well-isolated change to an input
  state machine already covered by unit tests on both sides of the seam
  (`enter_api_key_for`/`take_pending_api_key` in `app.rs`; `submit_selected_provider`/
  `handle_provider_selector_key` in `main.rs`); no host-swap, provider-transport, or streaming code is
  touched.
- If a future report shows the modal still not opening after this fix ships, that would mean either a
  different call path reaches `perform_connect` directly (not identified during this investigation)
  or a startup auto-connect path (`bootstrap_pool_host`) is being confused with the interactive
  `/connect` picker path this spec fixes — those are separate code paths and out of scope here.
