# connect-stored-key-modal Implementation Plan

**Goal:** Make selecting a keyed provider (`anthropic`, `gemini`, `openai`, `grok`, `deepseek`) in
`/connect` always open the API-key modal when a key is already stored, instead of silently
reconnecting with it — Enter on the empty modal still reuses the stored key (unchanged), typing a
replacement now actually reaches the code path that saves and connects with it, and the previous
`Alt+Enter`/`--rekey` escape hatch (fragile across terminals, never surfaced anywhere but two locale
strings) is retired since it no longer does anything distinct. Fixes `savvagent/otto#146`
(reopened).

**Architecture:** The live `/connect` flow is entirely plugin-driven
(`internal:connect` → `ConnectPickerScreen` → each provider plugin's private `connect <id>` slash →
`Effect::PromptApiKey` or `Effect::RegisterProvider`) — **not** the `App`-level `InputMode` state
machine in `crates/otto/src/main.rs`, which is dead code on every build that ships the Core
`internal:connect` plugin (always true today). The fix removes the silent-reconnect short-circuit
from the five keyed provider plugins' `handle_slash` (`crates/otto/src/plugin/builtin/
provider_{anthropic,gemini,openai,grok,deepseek}/mod.rs`), so they always emit
`Effect::PromptApiKey`; retires the now-redundant `Alt`-modifier distinction in
`ConnectPickerScreen::on_key` (`crates/otto/src/plugin/builtin/connect/screen.rs`); extracts the
modal-submit logic in `run_app` (`crates/otto/src/main.rs`) into a testable
`handle_api_key_modal_submit` helper (mirroring the existing `handle_provider_selector_key`/
`submit_selected_provider` split) since it's shared by every path that opens the modal; applies the
identical defense-in-depth fix to the dead legacy `submit_selected_provider` fallback; and updates
two locale strings, one doc comment/test in `providers.rs`, and `README.md` that referenced
`Alt+Enter`. No crate boundary changes — everything lands inside `crates/otto`. No host-swap
`RwLock` code is touched and no streaming provider path is touched.

**Tech Stack:** Existing workspace only — `crates/otto` (five provider plugin files, `connect`
plugin's `screen.rs`, `main.rs`, `providers.rs`, locale files, `README.md`). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-14-connect-stored-key-modal-design.md` — read it first,
including its "Revision note" and "Premise corrections" sections, which explain why the fix targets
the plugin files rather than `main.rs`'s legacy picker.

**Release line:** at least MINOR — this reverses part of a previously MINOR-shipped design decision
(issue #82's "silent stored-key reconnect + Alt+Enter re-key"), so it is classified the same way for
consistency (see the spec's "Public-interface changes"), not treated as a plain PATCH bug fix. Re-read
`workspace.package.version` at cut time (currently `0.30.8` as of branch creation) — Non-Negotiable
Rule 8 governs the actual batched release line, which is the highest bump anything in the batch
requires.

**Branch:** `connect/deepseek-freeze`

## File Map

**New files**
- `docs/superpowers/specs/2026-09-14-connect-stored-key-modal-design.md` — design source of truth
  (already committed, revised).
- `docs/superpowers/plans/2026-09-14-connect-stored-key-modal.md` — this plan (revised).

**Modified files**
- `crates/otto/src/plugin/builtin/provider_anthropic/mod.rs`
- `crates/otto/src/plugin/builtin/provider_gemini/mod.rs`
- `crates/otto/src/plugin/builtin/provider_openai/mod.rs`
- `crates/otto/src/plugin/builtin/provider_grok/mod.rs`
- `crates/otto/src/plugin/builtin/provider_deepseek/mod.rs`
  — `handle_slash`'s silent-reconnect short-circuit removed; always emit `Effect::PromptApiKey`;
  the `handle_slash_with_stored_key_skips_modal` test renamed/inverted per file;
  `handle_slash_with_rekey_flag_opens_modal_even_when_client_exists` kept, doc comment updated.
- `crates/otto/src/plugin/builtin/connect/screen.rs` — `Enter` handler drops the `Alt`-modifier
  distinction; `alt_enter_emits_rekey_slash` test replaced.
- `crates/otto/src/main.rs` — `submit_selected_provider`'s `Ok(Some(_))` arm (defense-in-depth);
  new `ApiKeySubmitAction` enum + `handle_api_key_modal_submit` helper; `run_app`'s
  `InputMode::EnteringApiKey`/`KeyCode::Enter` arm rewritten to call it; test updates.
- `crates/otto/src/providers.rs` — `turn_auth_hint`'s doc comment + test updated.
- `crates/otto/locales/{en,es,hi,pt}.toml` — `notes.connect-rejected-keyed`,
  `notes.turn-auth-failed-hint`, and `picker.connect.tips` drop the "Alt+Enter"/"Alt-Enter" wording.
- `README.md` — the `/connect` row.
- `CHANGELOG.md` — a `Changed` entry (added in the dedicated release PR per Non-Negotiable Rule 8 /
  Phase 4 step 12, not in this PR).

## Task 1: Always prompt for the API key in the five keyed provider plugins

**Files:**
- Modify: `crates/otto/src/plugin/builtin/provider_anthropic/mod.rs`
- Modify: `crates/otto/src/plugin/builtin/provider_gemini/mod.rs`
- Modify: `crates/otto/src/plugin/builtin/provider_openai/mod.rs`
- Modify: `crates/otto/src/plugin/builtin/provider_grok/mod.rs`
- Modify: `crates/otto/src/plugin/builtin/provider_deepseek/mod.rs`

Do all five files together (identical change, identical test rename) — do not split across
sub-steps per file; treat this as one mechanical, repeated edit so it can't drift between files.

- [ ] **Step 1: Write the failing tests first.** In each file's `#[cfg(test)] mod tests`, replace
  the existing `handle_slash_with_stored_key_skips_modal` test with (substitute the file's own
  `PROVIDER_ID`/plugin type name — e.g. `ProviderDeepSeekPlugin`/`"deepseek"` for the deepseek file):

  ```rust
  /// The picker dispatches `connect <id>` even with a stored key — this must
  /// now open the modal (confirm-or-replace), never register silently. See
  /// savvagent/otto#146 (reopened): the old shortcut connected immediately
  /// with no reliable, discoverable way to change the key in-session.
  #[tokio::test]
  #[serial_test::serial]
  async fn handle_slash_with_stored_key_opens_modal_for_confirm_or_replace() {
      use_mock_keyring();
      rust_i18n::set_locale("en");

      let _ = keyring::Entry::new("otto", PROVIDER_ID).map(|e| e.delete_credential());
      let _ = keyring::Entry::new("otto", PROVIDER_ID).map(|e| e.set_password("test-key"));

      let mut p = ProviderXPlugin::new(); // substitute the real type name
      let effs = p.handle_slash("connect <id>", vec![]).await.unwrap(); // substitute the real slash name
      let saw_prompt = effs.iter().any(
          |e| matches!(e, Effect::PromptApiKey { provider_id } if provider_id.as_str() == PROVIDER_ID),
      );
      assert!(
          saw_prompt,
          "a stored key must still open the modal, not silently reconnect; got effects: {effs:?}"
      );
      let saw_register = effs
          .iter()
          .any(|e| matches!(e, Effect::RegisterProvider { .. }));
      assert!(
          !saw_register,
          "must not register without user confirmation; got effects: {effs:?}"
      );

      let _ = keyring::Entry::new("otto", PROVIDER_ID).map(|e| e.delete_credential());
      rust_i18n::set_locale("en");
  }
  ```

  Update the doc comment above `handle_slash_with_rekey_flag_opens_modal_even_when_client_exists`
  (unchanged body — it still passes) to note it's now a redundant-but-harmless input:
  ```rust
  /// `--rekey` no longer changes `handle_slash`'s behavior (every stored-key
  /// case opens the modal now) — kept as a regression guard that passing it
  /// still opens the modal rather than erroring or being misinterpreted.
  ```

  Run (per crate, not per file — there's one `otto` test binary):
  ```bash
  cargo test -p otto handle_slash_with_stored_key_opens_modal_for_confirm_or_replace
  ```
  Expect **FAIL** for all five (current code still returns `RegisterProvider` on a stored key) —
  confirms the tests exercise the bug before the fix.

- [ ] **Step 2: Fix `handle_slash` in all five files.** Change:
  ```rust
  async fn handle_slash(
      &mut self,
      _: &str,
      args: Vec<String>,
  ) -> Result<Vec<Effect>, PluginError> {
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
  async fn handle_slash(
      &mut self,
      _: &str,
      _args: Vec<String>,
  ) -> Result<Vec<Effect>, PluginError> {
      // Always open the modal so the user can confirm the stored key (Enter
      // on the empty field, unchanged) or replace it (type a new key) —
      // never connect silently. See savvagent/otto#146 (reopened).
      Ok(vec![Effect::PromptApiKey {
          provider_id: ProviderId::new(PROVIDER_ID).expect("valid"),
      }])
  }
  ```
  Do **not** touch `try_connect_from_keyring` itself or the `HostEvent::HostStarting` arm in
  `on_event` — startup auto-reconnect must stay silent.

- [ ] **Step 3: Run the new tests.**
  ```bash
  cargo test -p otto handle_slash_with_stored_key_opens_modal_for_confirm_or_replace
  cargo test -p otto handle_slash_with_rekey_flag_opens_modal_even_when_client_exists
  cargo test -p otto no_creds_emits_prompt_api_key
  ```
  Expect all PASS across all five providers.

- [ ] **Step 4: Run each provider plugin's full test module** to confirm nothing else in the file
  (manifest tests, render_slot tests, on_event tests) regressed:
  ```bash
  cargo test -p otto plugin::builtin::provider_anthropic::
  cargo test -p otto plugin::builtin::provider_gemini::
  cargo test -p otto plugin::builtin::provider_openai::
  cargo test -p otto plugin::builtin::provider_grok::
  cargo test -p otto plugin::builtin::provider_deepseek::
  ```

- [ ] **Step 5: Public-interface note.** This changes `/connect`'s interactive behavior for a
  stored-key provider (always prompts instead of silently reconnecting) — classified as a
  MINOR-level "breaking" behavior change per the spec's "Public-interface changes" section,
  consistent with how issue #82 classified its own analogous change. No SPP wire type,
  `ProviderHandler`/`ProviderClient` method, tool MCP schema, plugin ABI surface, or on-disk
  transcript/keyring format is touched.

- [ ] **Step 6: Format and commit.**
  ```bash
  cargo fmt --all
  git add crates/otto/src/plugin/builtin/provider_anthropic/mod.rs \
          crates/otto/src/plugin/builtin/provider_gemini/mod.rs \
          crates/otto/src/plugin/builtin/provider_openai/mod.rs \
          crates/otto/src/plugin/builtin/provider_grok/mod.rs \
          crates/otto/src/plugin/builtin/provider_deepseek/mod.rs
  git commit -m "otto: always prompt for the API key when /connect finds one already stored"
  ```

## Task 2: Retire the Alt+Enter/--rekey distinction in the picker screen

**Files:**
- Modify: `crates/otto/src/plugin/builtin/connect/screen.rs`

- [ ] **Step 1: Write the failing test first.** Replace `alt_enter_emits_rekey_slash` (currently
  asserting `args == ["--rekey"]` on an Alt+Enter keypress) with:

  ```rust
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
  ```

  Run:
  ```bash
  cargo test -p otto alt_enter_routes_identically_to_plain_enter
  ```
  Expect **FAIL** (current code still emits `["--rekey"]` for Alt+Enter).

- [ ] **Step 2: Simplify the `Enter` handler.** Change:
  ```rust
  KeyCodePortable::Enter => {
      let Some((pid, _)) = self.selected_candidate().cloned() else {
          return Ok(vec![]);
      };
      let name = format!("connect {}", pid.as_str());
      let args = if key.modifiers.alt {
          vec!["--rekey".to_string()]
      } else {
          vec![]
      };
      Ok(vec![Effect::Stack(vec![
          Effect::CloseScreen,
          Effect::RunSlash { name, args },
      ])])
  }
  ```
  to:
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
  `key` may now be unused for its `.modifiers` field in this arm — check
  `cargo clippy -p otto --all-targets` in Step 4 for an unused-binding warning and prefix with `_`
  only if the compiler actually flags it (the outer `key: KeyEventPortable` parameter is still used
  for `key.code` in the outer `match`, so likely no warning).

- [ ] **Step 3: Run the new test.**
  ```bash
  cargo test -p otto alt_enter_routes_identically_to_plain_enter
  ```
  Expect PASS.

- [ ] **Step 4: Run the full `connect` plugin test module.**
  ```bash
  cargo test -p otto plugin::builtin::connect::
  ```
  Expect all PASS, including `enter_with_candidate_routes_to_connect_provider_slash`,
  `enter_uses_filtered_candidate`, and the rest of `screen.rs`'s existing suite, unaffected by this
  change.

- [ ] **Step 5: Format and commit.**
  ```bash
  cargo fmt --all
  git add crates/otto/src/plugin/builtin/connect/screen.rs
  git commit -m "otto: drop the now-redundant Alt+Enter rekey distinction in the connect picker"
  ```

## Task 3: Extract `handle_api_key_modal_submit`; defense-in-depth fix for the dead legacy path

**Files:**
- Modify: `crates/otto/src/main.rs`

- [ ] **Step 1: Write the failing tests first.** In `crates/otto/src/main.rs`'s
  `mod connect_provider_selector_tests` (currently starting around line 4708), replace
  `connect_provider_selector_enter_keyed_provider_uses_stored_key_before_prompting` with:

  ```rust
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
  ```

  Add a new module (near `connect_provider_selector_tests`) for the extracted helper:

  ```rust
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
              ApiKeySubmitAction::Connect { spec: got_spec, api_key } => {
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
              ApiKeySubmitAction::Connect { spec: got_spec, api_key } => {
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
  ```

  Run:
  ```bash
  cargo test -p otto connect_provider_selector_enter_keyed_provider_with_stored_key_opens_modal
  cargo test -p otto api_key_modal_submit_tests
  ```
  Expect the first to **FAIL** (old `submit_selected_provider` still short-circuits) and the second
  to **fail to compile** (`handle_api_key_modal_submit`/`ApiKeySubmitAction` don't exist yet).

- [ ] **Step 2: Fix `submit_selected_provider`** (currently around lines 4432-4449):
  ```rust
  match load_creds(spec) {
      Ok(Some(key)) => {
          app.input_mode = InputMode::Editing;
          app.push_note(
              rust_i18n::t!("notes.using-stored-key", name = spec.display_name).to_string(),
          );
          Some(PendingProviderConnect { spec, api_key: key })
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
  becomes:
  ```rust
  match load_creds(spec) {
      Ok(Some(_)) => {
          // A credential is already stored — open the modal instead of
          // connecting immediately. Defense-in-depth: this legacy fallback
          // is unreachable while the Core internal:connect plugin is
          // installed (see submit_selected_provider's own module context),
          // but it should stay consistent with the live plugin path. See
          // savvagent/otto#146.
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

- [ ] **Step 3: Add the `ApiKeySubmitAction` enum and `handle_api_key_modal_submit` helper.** Near
  `handle_provider_selector_key` (currently ending around line 4494):

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
                      rust_i18n::t!("notes.using-stored-key", name = spec.display_name)
                          .to_string(),
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

- [ ] **Step 4: Rewrite `run_app`'s `InputMode::EnteringApiKey`/`KeyCode::Enter` arm** (currently
  around lines 4225-4257) from:
  ```rust
  KeyCode::Enter => match app.take_pending_api_key() {
      Some((spec, Some(key))) => {
          app.input_mode = InputMode::Editing;
          perform_connect(spec, key, &host_slot, &project_root, &tool_bins, app)
              .await;
      }
      Some((spec, None)) => {
          match creds::load(spec.id) {
              Ok(Some(stored)) => {
                  app.cancel_connect();
                  perform_connect(
                      spec,
                      stored,
                      &host_slot,
                      &project_root,
                      &tool_bins,
                      app,
                  )
                  .await;
              }
              _ => {
                  app.push_note(rust_i18n::t!("notes.api-key-empty").to_string());
              }
          }
      }
      None => {}
  },
  ```
  to:
  ```rust
  KeyCode::Enter => {
      let action = handle_api_key_modal_submit(app, |spec| {
          creds::load(spec.id).map_err(|e| format!("{e:#}"))
      });
      match action {
          ApiKeySubmitAction::Connect { spec, api_key } => {
              perform_connect(spec, api_key, &host_slot, &project_root, &tool_bins, app)
                  .await;
          }
          ApiKeySubmitAction::NoStoredKey | ApiKeySubmitAction::Idle => {}
      }
  }
  ```

- [ ] **Step 5: Run the new/updated tests.**
  ```bash
  cargo test -p otto connect_provider_selector_enter_keyed_provider_with_stored_key_opens_modal
  cargo test -p otto api_key_modal_submit_tests
  cargo test -p otto connect_provider_selector_tests
  ```
  Expect all PASS.

- [ ] **Step 6: Run the full `otto` test suite.**
  ```bash
  cargo test -p otto
  ```
  Expect all PASS.

- [ ] **Step 7: Host-swap / streaming invariants — vacuously satisfied.** No
  `crates/otto/src/app.rs` or `crates/otto/src/tui.rs` code is modified (only called, unchanged).
  No streaming provider path touched.

- [ ] **Step 8: Format and commit.**
  ```bash
  cargo fmt --all
  git add crates/otto/src/main.rs
  git commit -m "otto: extract handle_api_key_modal_submit; fix the dead legacy connect fallback"
  ```

## Task 4: Update `turn_auth_hint` and the three Alt+Enter locale strings

**Files:**
- Modify: `crates/otto/src/providers.rs`
- Modify: `crates/otto/locales/en.toml`
- Modify: `crates/otto/locales/es.toml`
- Modify: `crates/otto/locales/hi.toml`
- Modify: `crates/otto/locales/pt.toml`

Note: this task also fixes `picker.connect.tips` — the connect picker screen's own footer,
rendered directly by `ConnectPickerScreen::tips()`
(`crates/otto/src/plugin/builtin/connect/screen.rs:233-238`). This is the most user-visible of the
three strings (it's the tips line on the exact screen Task 1/2 change) and was missed in an earlier
pass of this plan's spec because the Rust format-string literal never mentions "Alt+Enter" — only
the locale value it interpolates does. Re-grep every locale file for `"Alt"` after Step 2 below to
confirm no fourth instance was missed.

- [ ] **Step 1: Write the failing test first.** In `crates/otto/src/providers.rs`'s
  `turn_auth_hint_only_for_known_keyed_providers` test, change:
  ```rust
  assert!(
      text.contains("Alt+Enter"),
      "hint should mention Alt+Enter: {text}"
  );
  ```
  to:
  ```rust
  assert!(
      !text.contains("Alt+Enter"),
      "hint should no longer mention Alt+Enter (every stored-key /connect \
       selection opens the modal now): {text}"
  );
  ```
  Run:
  ```bash
  cargo test -p otto turn_auth_hint_only_for_known_keyed_providers
  ```
  Expect **FAIL** (locale string still contains "Alt+Enter").

- [ ] **Step 2: Update the locale strings.** In each of `crates/otto/locales/{en,es,hi,pt}.toml`,
  change `notes.turn-auth-failed-hint`, `notes.connect-rejected-keyed`, and `picker.connect.tips`
  (under the `[picker.connect]` section) to drop the "Alt+Enter"/"Alt-Enter" wording:

  `en.toml`:
  ```
  connect-rejected-keyed        = "Connect to %{id} failed: %{err} Run /connect and pick %{id} to try a different key."
  turn-auth-failed-hint         = "Run /connect and pick %{name} to enter a different API key."
  tips                  = "↑/↓ navigate · Enter connect · Esc cancel"
  ```
  `es.toml`:
  ```
  connect-rejected-keyed        = "La conexión con %{id} falló: %{err} Ejecuta /connect y selecciona %{id} para probar otra clave."
  turn-auth-failed-hint         = "Ejecuta /connect y selecciona %{name} para introducir una clave de API distinta."
  tips                  = "↑/↓ navegar · Enter conectar · Esc cancelar"
  ```
  `hi.toml`:
  ```
  connect-rejected-keyed        = "%{id} से कनेक्ट विफल: %{err} अलग कुंजी आज़माने के लिए /connect चलाएं और %{id} को चुनें।"
  turn-auth-failed-hint         = "%{name} के लिए अलग API कुंजी दर्ज करने हेतु /connect चलाएं और %{name} को चुनें।"
  tips                  = "↑/↓ नेविगेट · Enter कनेक्ट · Esc रद्द"
  ```
  `pt.toml`:
  ```
  connect-rejected-keyed        = "Falha ao conectar com %{id}: %{err} Execute /connect e selecione %{id} para tentar outra chave."
  turn-auth-failed-hint         = "Execute /connect e selecione %{name} para informar uma chave de API diferente."
  tips                  = "↑/↓ navegar · Enter conectar · Esc cancelar"
  ```
  Keep each key's existing column alignment/formatting in its file — match the surrounding lines'
  `=` alignment rather than the exact spacing shown above. `picker.connect.tips` has no
  placeholders (`%{...}`) before or after this edit, so the `locales` integration test's
  per-key-placeholder-set check is unaffected.

  After editing, re-grep for any missed instance:
  ```bash
  grep -rn "Alt+Enter\|Alt-Enter" crates/otto/locales/*.toml
  ```
  Expect no output.

- [ ] **Step 3: Update `turn_auth_hint`'s doc comment** (currently ending "...pressing `Alt+Enter`
  to enter a fresh key.") to: "...and entering (or reusing) a key in the modal that opens."

- [ ] **Step 4: Run the test again.**
  ```bash
  cargo test -p otto turn_auth_hint_only_for_known_keyed_providers
  ```
  Expect PASS.

- [ ] **Step 5: Run the locale integration test** (this repo has one — confirm all locale files stay
  in sync):
  ```bash
  cargo test -p otto --test locales
  ```
  Expect PASS — this test suite typically checks every locale file has the same key set; changing
  values (not keys) in all four files together should not break it, but run it to confirm.

- [ ] **Step 6: Format and commit.**
  ```bash
  cargo fmt --all
  git add crates/otto/src/providers.rs crates/otto/locales/en.toml crates/otto/locales/es.toml \
          crates/otto/locales/hi.toml crates/otto/locales/pt.toml
  git commit -m "otto: drop the Alt+Enter mention from connect recovery hints"
  ```

## Task 5: Correct README.md's `/connect` documentation

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Replace the `/connect` row** (currently line 122):
  ```
  | `/connect` | Open the provider picker to add a provider to the connection pool. Silent when the keyring already has a stored key — the API-key modal only opens when a key is missing, or when pressed with `Alt+Enter` to re-key. Multiple providers can be connected simultaneously; switch with `/use <provider>`. |
  ```
  with:
  ```
  | `/connect` | Open the provider picker to add a provider to the connection pool. If the selected provider already has a stored key, the API-key modal opens with a "press Enter to reuse, or paste a new key" placeholder — press Enter on the empty field to keep using the stored key, or type a replacement to save and connect with a new one. Multiple providers can be connected simultaneously; switch with `/use <provider>`. |
  ```

- [ ] **Step 2: Grep for any other README.md reference to `Alt+Enter` or the old "silent" wording**
  (`grep -n "Alt+Enter\|Silent when the keyring" README.md`) and correct it too if found; expect
  none beyond the row just edited.

- [ ] **Step 3: Commit.**
  ```bash
  git add README.md
  git commit -m "docs: correct /connect stored-key behavior in README"
  ```

## Task 6: Cut the release (notes only — performed per Phase 4 step 12, not in this PR)

**Files:** none in this PR.

- [ ] **Step 1:** This PR does **not** bump `workspace.package.version` and does **not** add a
  `CHANGELOG.md` section — that happens in the dedicated release PR after this merges, per
  Non-Negotiable Rule 8 / Phase 4 step 12. Re-read `workspace.package.version` at cut time (it may
  have moved past `0.30.8` if another PR merges first) and cut at least the next MINOR (or higher
  if something else in the batch requires more) from whatever it then reads — this PR's own floor is
  MINOR, per the spec's "Public-interface changes" classification, not PATCH. The `CHANGELOG.md`
  entry: a `Changed` bullet along the lines of "`/connect` now always opens the API-key modal
  (reuse-or-replace placeholder) when the selected provider already has a stored key, instead of
  silently reconnecting with it; the previous `Alt+Enter` re-key escape hatch is retired since it no
  longer does anything the default flow doesn't already do."
