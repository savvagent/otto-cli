# `/connect` silently reconnects with a stored key instead of prompting — design

Date: 2026-09-14
Status: IMPLEMENTED
Source: savvagent/otto#146 (reopened)
Related: supersedes part of the behavior shipped by
`docs/superpowers/specs/2026-09-09-issue-82-connect-picker-only-design.md` (issue #82) — see
"Premise corrections" below.

## Revision note

This spec's first draft investigated the wrong code path (`crates/otto/src/main.rs`'s
`submit_selected_provider`/`InputMode::SelectingProvider` state machine) and was approved by two
review rounds before that mistake was caught by further investigation. That code path is **dead in
production** (see "Premise corrections"). This revision replaces the entire Problem/Approach/Scope
with the correct root cause; the two earlier review rounds are void because they validated an
analysis of code the live `/connect` flow never reaches.

## Problem

`savvagent/otto#146` was originally filed against a TUI freeze (raw MCP/`tracing` log output
rendered into the transcript pane on a mid-session DeepSeek `/connect`) and was fixed by #163
(`spawn_tool_transport`, shipped in v0.30.5). The issue owner (`robhicks`) reopened it on
2026-09-14 after confirming the freeze is gone but reporting a second, distinct defect on the exact
same repro path: **selecting a provider that already has a stored API key never opens the API-key
entry modal via plain `Enter`** — `/connect` → pick DeepSeek → Enter silently reconnects with
whatever key is already in the OS keyring, with no discoverable way to type a replacement key in
that session.

### Root cause (confirmed by reading the current code and its own tests, not inferred)

The live `/connect` flow is entirely plugin-driven, **not** the `App`-level `InputMode` state
machine in `crates/otto/src/main.rs` that an earlier draft of this spec targeted:

1. `ConnectPlugin::handle_slash` (`crates/otto/src/plugin/builtin/connect/mod.rs:80-93`) — the
   `internal:connect` Core plugin, always installed — unconditionally emits
   `Effect::OpenScreen { id: "connect.picker", .. }` for `/connect`, regardless of arguments. Per
   its own doc comment: "the arg-routing path... was removed in issue #82 to unify on a single
   entry-point."
2. `ConnectPickerScreen::on_key` (`crates/otto/src/plugin/builtin/connect/screen.rs:214-228`)
   handles `Enter` on the highlighted candidate by emitting
   `Effect::RunSlash { name: "connect {pid}", args }`, where `args` is `["--rekey"]` only when the
   keypress carried the `Alt` modifier, else empty.
3. That `connect <id>` slash is a **private, internal namespace** (`connect::
   is_internal_connect_namespace`, matched by the `"connect "` prefix — never shown in the `/`
   palette) owned by each of the five keyed provider plugins
   (`provider_{anthropic,gemini,openai,grok,deepseek}/mod.rs`). Their identical `handle_slash`
   bodies:
   ```rust
   let rekey = args.iter().any(|a| a == "--rekey");
   if !rekey && self.try_connect_from_keyring().is_some() {
       // Stored key worked; register without opening the modal.
       return Ok(vec![Effect::RegisterProvider { id: ..., display_name: ... }]);
   }
   // No stored key, --rekey explicitly requested, or stored key
   // didn't yield a working client: open the modal.
   Ok(vec![Effect::PromptApiKey { provider_id: ... }])
   ```
   This is the actual short-circuit `savvagent/otto#146`'s reopening comment describes — just
   located in five provider-plugin files, not in `main.rs`.
4. `Effect::PromptApiKey`'s handler (`crates/otto/src/plugin/effects.rs:219-236`) already does the
   right thing once it fires: it computes `has_stored` from the keyring and calls
   `app.enter_api_key_for(spec, has_stored)`, which is what shows the "press Enter to reuse, or
   paste a new key" placeholder (`enter_api_key_for`, `crates/otto/src/app.rs:1833-1849`, already
   correct and already tested via `provider_selector_api_key_entry_helper_preserves_placeholder_behavior`,
   `crates/otto/src/app.rs:2718-2744`). **This part of the pipeline is not the defect.**
5. The modal's own `Enter` handling (`run_app`'s `InputMode::EnteringApiKey` arm,
   `crates/otto/src/main.rs:4223-4261`) is *also* already correct and is shared by every path that
   ever opens the modal (there is exactly one `InputMode::EnteringApiKey`): an empty submit falls
   back to the stored credential via `creds::load`; a typed submit calls `perform_connect` with the
   typed value, which unconditionally persists it via `creds::save` before building the
   registration (`crates/otto/src/main.rs:2794-2802`), correctly replacing the stored key.

So steps 4–5 — the modal itself, and "Enter reuses / type replaces" — are already correct and
already live. **The single defect is step 3**: five provider plugins' `handle_slash` never reach
step 4 for a plain (non-`--rekey`) selection when a key is already stored, because they intercept
and return `Effect::RegisterProvider` first.

### Why `Alt+Enter`/`--rekey` doesn't close this gap in practice

Step 2/3's `--rekey` mechanism is real, wired, tested (`alt_enter_emits_rekey_slash`,
`crates/otto/src/plugin/builtin/connect/screen.rs:304-326`; `handle_slash_with_rekey_flag_opens_modal_even_when_client_exists`,
present in all five provider plugin files), and documented — in `README.md:122` and in a live,
user-facing recovery hint (`notes.turn-auth-failed-hint`, shown on a turn-time auth failure:
*"Run /connect and pick %{name} with Alt+Enter to enter a different API key."*). The reopening
comment's claim that "there is no keybinding, flag, or menu item that forces the modal when a key
is already stored" is **factually incorrect as written** — a keybinding exists — but the underlying
complaint is still valid: `Alt+Enter` is a modifier-chord on `Enter`, and modifier-chord detection
on that specific combination is exactly the class of input that many terminal emulators, multiplexers
(tmux/screen), and SSH sessions do not reliably forward to the application without an opt-in
"enhanced keyboard" protocol (kitty keyboard protocol / `CSI u`) that otto does not require and most
terminals don't enable by default — the same keystroke can silently arrive at otto as a plain
`Enter` with no `Alt` bit set. This plausibly explains both halves of the report: the code is
correct, and a real user in a real terminal can still have no working way to reach it. A
mechanism gated behind a terminal-dependent modifier chord is not a **reliable** "menu item, flag,
or keybinding" from the reporting user's vantage point, even though it exists in source.

### Why this wasn't caught by review sooner

Every one of the five provider plugins carries its own passing test
(`handle_slash_with_stored_key_skips_modal`) that asserts *exactly* the behavior this spec changes:
that a stored key causes an immediate `RegisterProvider`, never a `PromptApiKey`. These tests are
not wrong readings of the code — they accurately pinned down an intentional, previously-designed
behavior (see "Premise corrections" below) — but their names describe it as correct, so a green
suite never flagged it as the live-reachability gap this spec fixes.

## Premise corrections

- **The reopening comment's cited code location is wrong.** It names `crates/otto/src/main.rs`'s
  `submit_selected_provider` (~line 4419) as the root cause. That function belongs to
  `App::handle_command`'s `"/connect"` arm (`crates/otto/src/app.rs:1710-1724`), documented in its
  own comment as "still partially routed through the legacy `SelectingProvider` InputMode flow when
  no plugin owns the slash... this arm only fires if a future build removes that [Core] plugin." The
  `internal:connect` plugin is `PluginKind::Core` and always installed, so this arm — and therefore
  `submit_selected_provider`, `handle_provider_selector_key`, `InputMode::SelectingProvider` — is
  **dead code on every real `/connect` invocation today**. The actual defect is in the five provider
  plugins' `handle_slash` (see Root Cause above). This correction does not change the acceptance
  criteria, which describe user-visible behavior, not a file path.
- **This fix deliberately reverses part of a previous, intentional, shipped design decision** —
  `docs/superpowers/specs/2026-09-09-issue-82-connect-picker-only-design.md` (issue #82, shipped
  under a MINOR bump). That spec's own Goal & Success Criteria explicitly required: "In the picker,
  Enter still connects the highlighted provider and `Alt+Enter` still opens the API-key modal for
  it; the silent stored-key reconnect still happens without a modal." This spec changes that: the
  silent stored-key reconnect (no modal) goes away entirely; every keyed-provider selection with a
  stored key now opens the modal, matching `savvagent/otto#146`'s updated acceptance criteria.
  This is not an oversight or a silent contradiction — it's a deliberate, explicit supersession
  recorded here because the reporting user (repo owner) determined the #82 behavior doesn't serve
  real terminal usage reliably. Given the precedent that #82's own analogous change (removing the
  typed `/connect <provider>` command) was treated as a MINOR-level breaking behavior change, this
  spec classifies its reversal the same way — see "Public-interface changes" below.
- **The dead `main.rs` legacy path (`submit_selected_provider`) shares the identical bug pattern**
  (its `Ok(Some(key))` arm also connects immediately on a stored key with no modal). Per
  Stop-and-escalate guidance, discovering the same bug pattern elsewhere is ordinarily a
  follow-up-issue matter, not a scope-widener — but this instance is cheap, already fully designed,
  and touches a function this spec's plan already needs to touch for an unrelated, necessary reason
  (Task 2 extracts `handle_api_key_modal_submit` from the same file for testability). Fixing it now,
  for defense-in-depth consistency in case that fallback is ever exercised (e.g. a future build that
  disables the Core connect plugin), costs one small diff already reviewed once. Folded into Task 2
  rather than filed separately.

## Approach

### 1. Remove the silent stored-key short-circuit from the five keyed provider plugins

In each of `crates/otto/src/plugin/builtin/provider_{anthropic,gemini,openai,grok,deepseek}/mod.rs`,
`handle_slash` changes from:

```rust
async fn handle_slash(&mut self, _: &str, args: Vec<String>) -> Result<Vec<Effect>, PluginError> {
    let rekey = args.iter().any(|a| a == "--rekey");
    if !rekey && self.try_connect_from_keyring().is_some() {
        // Stored key worked; register without opening the modal.
        return Ok(vec![Effect::RegisterProvider {
            id: ProviderId::new(PROVIDER_ID).expect("valid"),
            display_name: DISPLAY_NAME.into(),
        }]);
    }
    // No stored key, --rekey explicitly requested, or stored key
    // didn't yield a working client: open the modal.
    Ok(vec![Effect::PromptApiKey {
        provider_id: ProviderId::new(PROVIDER_ID).expect("valid"),
    }])
}
```

to:

```rust
async fn handle_slash(&mut self, _: &str, _args: Vec<String>) -> Result<Vec<Effect>, PluginError> {
    // Always open the modal so the user can confirm the stored key (Enter
    // on the empty field, unchanged) or replace it (type a new key) —
    // never connect silently. The previous `!rekey && stored key works`
    // shortcut connected immediately with no discoverable, terminal-
    // reliable way to change the key in the same session. See
    // savvagent/otto#146 (reopened) and this file's design spec.
    Ok(vec![Effect::PromptApiKey {
        provider_id: ProviderId::new(PROVIDER_ID).expect("valid"),
    }])
}
```

`try_connect_from_keyring` itself is **not removed** — it's still the correct mechanism for
`HostEvent::HostStarting`'s startup auto-reconnect (`on_event`'s `HostStarting` arm in every one of
these files), which has no interactive picker involved and must stay silent. Only the
`handle_slash` (picker-triggered) call site changes.

`provider_local` (keyless) is untouched — it has no credential to confirm or replace.

### 2. Retire the now-redundant `Alt+Enter`/`--rekey` distinction in the picker

Once every stored-key selection opens the modal unconditionally, `--rekey` no longer changes
`handle_slash`'s behavior — keeping the modifier-chord plumbing around would leave dead-in-spirit
code (a keybinding that "does something" but no longer does anything *different*) and a
now-inaccurate `Alt+Enter` claim in two locale strings, one doc comment, and one test assertion.
`ConnectPickerScreen::on_key`'s `Enter` arm
(`crates/otto/src/plugin/builtin/connect/screen.rs:214-228`) drops the modifier check:

```rust
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
```

### 3. Make the modal-submit logic (already correct) independently testable

To satisfy the updated acceptance criteria's explicit request for "a regression test covering...
the path" of reusing vs. replacing a stored key, extract the inline `KeyCode::Enter` match arm
currently in `run_app`'s `InputMode::EnteringApiKey` handling
(`crates/otto/src/main.rs:4223-4261`) — which closes over `host_slot`/`project_root`/`tool_bins` and
calls the async `perform_connect` directly, so it isn't unit-testable in isolation today — into a
pure(ish) helper, mirroring this file's own established
`handle_provider_selector_key`/`submit_selected_provider` split:

```rust
#[derive(Debug)]
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

`run_app`'s arm becomes:

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

This refactor preserves the existing branch structure and ordering of already-correct, already-live
control flow — same branches, same order, same `perform_connect` call — but is not fully
behavior-neutral. The one behavioral addition is the `notes.using-stored-key` push on the
empty-submit-reuses-stored-key branch: today that note is only ever pushed by `submit_selected_provider`'s
dead `Ok(Some(key))` arm (step 4 removes it there too), so on the *live* path this is a small,
new-but-harmless user-visible note (previously the empty-submit reuse path pushed no note at all),
not a risk-free relocation of existing live behavior. Called out explicitly so the implementer
doesn't file it as a no-behavior-change refactor. No new async boundary, no new `.await` under a
lock, no change to `perform_connect`.

### 4. Defense-in-depth: fix the dead legacy fallback identically

In `submit_selected_provider` (`crates/otto/src/main.rs:4419-4450`), the `Ok(Some(key))` arm changes
from immediately returning `Some(PendingProviderConnect { spec, api_key: key })` to calling
`app.enter_api_key_for(spec, true)` and returning `None` — mirroring the fix in step 1, for the one
hypothetical case (Core connect plugin absent) where this fallback would ever run.

### 5. Update the strings that instructed users to use `Alt+Enter`

Three locale keys, in all four locale files (`en`, `es`, `hi`, `pt`), drop the now-unnecessary
`Alt+Enter`/`Alt-Enter` wording — plain `/connect` + pick is enough now:

- `notes.connect-rejected-keyed` and `notes.turn-auth-failed-hint` drop the "with Alt+Enter" clause.
- **`picker.connect.tips`** — the connect picker screen's own footer, rendered directly by
  `ConnectPickerScreen::tips()` (`crates/otto/src/plugin/builtin/connect/screen.rs:233-238`,
  `rust_i18n::t!("picker.connect.tips")`) — currently reads (en) "↑/↓ navigate · Enter connect ·
  Alt-Enter re-enter API key · Esc cancel" (and the equivalent in `es`/`hi`/`pt`). This is the most
  user-visible of the three: it is the tips line shown on the exact screen this fix changes, and an
  earlier pass of this spec missed it (caught in review) because the Rust format-string literal in
  `screen.rs` never mentions "Alt+Enter" directly — only the locale value it interpolates does. Drop
  the "Alt-Enter re-enter API key ·" segment, leaving "↑/↓ navigate · Enter connect · Esc cancel"
  (and equivalents).

`crates/otto/src/providers.rs`'s `turn_auth_hint` doc comment and its test
(`turn_auth_hint_only_for_known_keyed_providers`, currently asserting the text *contains*
"Alt+Enter") are updated to match — the test now asserts the text does **not** contain "Alt+Enter",
mirroring the existing `!text.contains("--rekey")` assertion already in that test. After all edits,
re-grep every locale file for `"Alt"` to confirm no fourth instance was missed.

### 6. Correct README.md

`README.md`'s `/connect` row (line 122) — currently "Silent when the keyring already has a stored
key — the API-key modal only opens when a key is missing, or when pressed with `Alt+Enter` to
re-key" — becomes:

> `/connect` | Open the provider picker to add a provider to the connection pool. If the selected
> provider already has a stored key, the API-key modal opens with a "press Enter to reuse, or paste
> a new key" placeholder — press Enter on the empty field to keep using the stored key, or type a
> replacement to save and connect with a new one. Multiple providers can be connected
> simultaneously; switch with `/use <provider>`.

## Scope

**In:**
- `crates/otto/src/plugin/builtin/provider_{anthropic,gemini,openai,grok,deepseek}/mod.rs` —
  `handle_slash`'s short-circuit removed; the corresponding
  `handle_slash_with_stored_key_skips_modal` test renamed and its assertions inverted per file;
  `handle_slash_with_rekey_flag_opens_modal_even_when_client_exists` kept (still passes, doc comment
  updated to note it's now a redundant-but-harmless input rather than a distinct code path).
- `crates/otto/src/plugin/builtin/connect/screen.rs` — the `Alt`-modifier branch removed from the
  `Enter` handler; `alt_enter_emits_rekey_slash` test replaced with one confirming Alt+Enter routes
  identically to plain Enter (no `--rekey` arg emitted).
- `crates/otto/src/main.rs` — `submit_selected_provider`'s `Ok(Some(_))` arm (defense-in-depth fix);
  new `ApiKeySubmitAction` enum + `handle_api_key_modal_submit` helper; `run_app`'s
  `InputMode::EnteringApiKey`/`KeyCode::Enter` arm rewritten to call it; the existing
  `connect_provider_selector_enter_keyed_provider_uses_stored_key_before_prompting` test
  renamed/rewritten; new tests for the extracted helper.
- `crates/otto/src/providers.rs` — `turn_auth_hint`'s doc comment and test updated.
- `crates/otto/locales/{en,es,hi,pt}.toml` — `notes.connect-rejected-keyed`,
  `notes.turn-auth-failed-hint`, and `picker.connect.tips` drop the "Alt+Enter"/"Alt-Enter" wording.
- `README.md` — the `/connect` row.
- `CHANGELOG.md` — a `Changed`/`Fixed` entry (added in the dedicated release PR per Non-Negotiable
  Rule 8 / Phase 4 step 12, not in this PR).

**Out:**
- Anything already fixed by #163 (`spawn_tool_transport`) — confirmed still correct, not re-touched.
- `App::enter_api_key_for`, `App::take_pending_api_key`, `App::cancel_connect`
  (`crates/otto/src/app.rs`) — already correct and already tested.
- `Effect::PromptApiKey`'s handler in `crates/otto/src/plugin/effects.rs` — already correctly
  computes `has_stored` and calls `enter_api_key_for`; not touched.
- `perform_connect`, `bootstrap_first_pool_host`, `apply_pending_pool_add` — unaffected; still
  receive a `(spec, api_key)` pair exactly as before.
- `try_connect_from_keyring` in the five provider plugins — kept, still used by
  `HostEvent::HostStarting`'s startup auto-reconnect.
- `provider_local` — keyless, no credential to confirm/replace.
- `notes.connect-already`, `notes.startup-build-failed`, `notes.startup-timeout`,
  `notes.use-not-connected` — none of these mention `Alt+Enter`; not touched.
- The host-swap `RwLock` discipline, the provider transport split, `ToolRegistry` stdio plumbing, or
  the `ProgressDispatcher` forwarder-abort pattern — none of this is touched.
- `command_palette`'s `connect <id>`-namespace filter (`is_internal_connect_namespace`) — unaffected;
  the namespace is still private/internal and still filtered from the palette the same way.

## Public-interface changes

**Breaking (user-facing interactive behavior), consistent with how #82 classified its own analogous
change.** `/connect` remains the one documented slash command (no rename/removal), but a keyed
provider selection with a stored credential now always requires a second keystroke (Enter on the
modal, or a typed replacement) instead of connecting on the first Enter — the "silent stored-key
reconnect" behavior that `docs/superpowers/specs/2026-09-09-issue-82-connect-picker-only-design.md`
explicitly specified is removed. `Alt+Enter`/`--rekey` stop being a distinct code path (harmless to
still press Alt+Enter — it now behaves exactly like plain Enter). Two locale strings and one
README row change their wording. None of Non-Negotiable Rule 6's example categories (tool schema,
`StreamEvent` shape, on-disk transcript/keyring format, plugin ABI) are touched, and the `/connect`
command itself is not renamed or removed — but given the precedent that #82 treated its own
picker-behavior change as MINOR-level breaking, this spec classifies its reversal the same way for
consistency: a `Changed` `CHANGELOG.md` entry (not merely `Fixed`), flagged explicitly to the
architect reviewer, and at least a MINOR version-line floor at release time (Non-Negotiable Rule 8
still governs the actual batched release line at cut time).

**Correction recorded at record-as-shipped time:** the release PR's mandatory architecture review
found this MINOR classification didn't hold up — #82's actual breaking change was *removing* the
`/connect <provider>` command surface, which this fix doesn't repeat, and this fix's own paragraph
above already states no Rule 6 category is touched. It shipped as v0.30.9, a `Fixed` PATCH entry.
See the implementation plan's "Shipped as" note for the full correction.

## Assumptions

- **The issue's reopening comment is the current, authoritative AC for this task, but its cited root
  cause is not** — the AC ("opens the modal instead of an immediate silent reconnect", "Enter on
  empty reuses", "typed key replaces") is a description of desired user-visible behavior, which
  this spec satisfies at the correct code location once premise-corrected (see above).
- **This is a deliberate reversal of `#82`'s "Alt+Enter re-key" design, not an oversight.** The
  terminal-reliability argument (Root Cause section) is offered as the most likely explanation for
  why a real user found the existing mechanism unusable; it is not independently reproduced against
  a specific terminal, but the fix it motivates (never gate re-keying behind a modifier chord) is
  correct regardless of whether that specific explanation is exactly right.
- **Removing the `Alt+Enter`/`--rekey` code paths, rather than merely making them redundant-but-
  present, is the right cleanup**, because leaving inert modifier-chord plumbing and outdated
  "press Alt+Enter" guidance in two locale strings and a README row would be actively misleading
  once every stored-key selection already opens the modal unconditionally.
- **The defense-in-depth fix to the dead `main.rs` legacy fallback is in scope** because it's cheap,
  in a file this plan already touches for an unrelated required reason, and keeps the codebase
  consistent — not because that path is reachable today.
- **No change to `perform_connect`, `Effect::PromptApiKey`'s handler, or `enter_api_key_for`/
  `take_pending_api_key`** — all three are already correct; the defect was purely in *when*
  `Effect::PromptApiKey` gets emitted.

## Goal & Success Criteria

Selecting a keyed provider in `/connect` that already has a stored key always opens the API-key
modal (pre-filled placeholder advertising "press Enter to reuse, or paste a new key") instead of
connecting immediately; pressing Enter on the empty field reuses the stored key exactly as before;
typing a new key and submitting replaces the stored credential and connects with it.

- None of the five keyed provider plugins' `handle_slash` ever returns `Effect::RegisterProvider`
  directly anymore — every invocation returns `Effect::PromptApiKey`.
- A renamed/updated unit test per provider plugin proves a stored key still opens the modal
  (`Effect::PromptApiKey`), never `Effect::RegisterProvider`.
- A new/updated unit test in `crates/otto/src/main.rs` proves: (a) `handle_api_key_modal_submit`
  reuses the stored key on an empty submit; (b) it connects with a typed replacement, ignoring any
  stored value; (c) the dead legacy `submit_selected_provider` fallback also opens the modal rather
  than connecting immediately.
- `crates/otto/src/plugin/builtin/connect/screen.rs`'s `Enter` handling no longer distinguishes the
  `Alt` modifier.
- `README.md`'s `/connect` row and the two locale strings no longer mention `Alt+Enter`.
- `cargo test -p otto` passes, including the new/updated tests.
- `cargo build --workspace --all-targets`, `cargo clippy --workspace --all-targets`, and
  `cargo fmt --all --check` are clean.

## Error Handling & Edge Cases

- **Keyring read error inside `Effect::PromptApiKey`'s handler.** Unchanged — `has_stored` is
  computed as `matches!(creds::load(spec.id), Ok(Some(_)))`, so a read error is treated the same as
  "no stored key" (placeholder without the "use stored" hint). Not touched by this fix.
- **Keyless providers (`provider_local`).** Unaffected — no credential to confirm or replace.
- **Startup auto-reconnect (`HostEvent::HostStarting`).** Unaffected — still silent, still uses
  `try_connect_from_keyring` directly, no picker/modal involved.
- **`Esc` inside the modal.** Unchanged: `app.cancel_connect()` still aborts back to `Editing`.
- **A user still presses `Alt+Enter` out of habit.** Harmless — routes identically to plain `Enter`
  now (opens the modal, same as any other keyed provider with a stored key).

## Risks & Open Questions

- **The terminal-reliability explanation for why `Alt+Enter` failed for the reporting user is
  plausible but not independently confirmed** (no specific terminal/multiplexer was identified).
  If a future report shows a different reason `Alt+Enter` seemed unreachable, that would not change
  this fix's correctness — the AC is satisfied by never depending on that keybinding at all — but it
  would be worth noting in a follow-up if the true cause turns out to be a otto-side input-handling
  bug rather than terminal limitations.
- **This reverses part of a deliberate, reviewed design (#82).** If a future report says the
  now-mandatory modal is *itself* undesired friction for some workflow (e.g. a fully scripted/headless
  `/connect` driver that relied on the old single-keystroke silent reconnect), that is a new,
  separate concern to raise as its own issue rather than grounds to partially revert this fix.
