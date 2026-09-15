# connect-rekey-already-registered Implementation Plan

**Goal:** Typing a new API key for a provider that's already connected and submitting it through
the `/connect` confirm-or-replace modal actually swaps the active session's client to the new key,
instead of silently keeping the stale client and telling the user to run `/disconnect` manually
first. Fixes `savvagent/otto#179`.

**Architecture:** `perform_connect` (`crates/otto/src/main.rs`) already builds a fresh, validated
`ProviderRegistration` from the freshly-typed (and freshly-keyring-saved) key before it ever touches
the pool; the defect is purely in what it does with that registration once it discovers the
provider's id is already in `Host`'s connection pool (`pool: RwLock<HashMap<ProviderId, PoolEntry>>`
in `crates/otto-host/src/session.rs`) — today it calls the additive-only `add_provider`, which
correctly refuses to clobber an existing entry, and `perform_connect` gives up with a note instead of
retrying. The fix adds one new pool API, `Host::replace_provider`, built entirely from the existing
`remove_provider`/`add_provider` primitives (no new lock, no new state), and points
`perform_connect` at it instead. No crate boundary changes — the new method lives in `otto-host`
alongside the pool it manages; `crates/otto` only changes which method it calls and drops one
now-dead locale key. No host-swap `RwLock` code (`app.rs`/`tui.rs`) or streaming provider path is
touched.

**Tech Stack:** Existing workspace only — `crates/otto-host` (new pool method + tests),
`crates/otto` (`main.rs`, four locale files). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-14-connect-rekey-already-registered-design.md` — read it
first, including "Approach" (the `replace_provider` doc comment carries the load-bearing race
reasoning verbatim) and "Error Handling & Edge Cases" (two distinct races, only one of which is
swallowed).

**Release line:** at least PATCH — a pure bug fix (`Fixed` `CHANGELOG.md` entry per the spec's
"Public-interface changes" section), no breaking or additive-but-flagged public-interface surface.
Re-read `workspace.package.version` at cut time; Non-Negotiable Rule 8 governs the actual batched
release line.

**Branch:** `host/connect-rekey`

## File Map

**New files**
- `docs/superpowers/specs/2026-09-14-connect-rekey-already-registered-design.md` — design source of
  truth (already committed, approved).
- `docs/superpowers/plans/2026-09-14-connect-rekey-already-registered.md` — this plan.

**Modified files**
- `crates/otto-host/src/session.rs` — new `Host::replace_provider` method.
- `crates/otto-host/tests/pool_lifecycle.rs` — new regression tests.
- `crates/otto/src/main.rs` — `perform_connect`'s `AlreadyRegistered`-handling branch replaced with a
  `replace_provider` call.
- `crates/otto/locales/en.toml`, `es.toml`, `hi.toml`, `pt.toml` — `notes.connect-already` removed
  (now unreferenced).
- `CHANGELOG.md` — a `Fixed` entry (added in the dedicated release PR per Non-Negotiable Rule 8 /
  Phase 4 step 12, not in this PR).

## Task 1: Add `Host::replace_provider` and its regression tests

**Files:**
- Modify: `crates/otto-host/src/session.rs`
- Modify: `crates/otto-host/tests/pool_lifecycle.rs`

- [ ] **Step 1: Add a distinguishable-client test double to `pool_lifecycle.rs`.** Near the existing
  `EchoClient`, add:

  ```rust
  /// A provider client whose `complete` response is tagged with a fixed
  /// string, so a test can prove *which* client answered a turn (unlike
  /// `EchoClient`, whose response never reflects which registration built
  /// it).
  struct TaggedClient(&'static str);

  #[async_trait]
  impl ProviderClient for TaggedClient {
      async fn complete(
          &self,
          req: CompleteRequest,
          _events: Option<mpsc::Sender<StreamEvent>>,
      ) -> Result<CompleteResponse, ProviderError> {
          Ok(CompleteResponse {
              id: format!("{}-0", self.0),
              model: req.model.clone(),
              content: vec![otto_protocol::ContentBlock::Text {
                  text: self.0.into(),
              }],
              stop_reason: otto_protocol::StopReason::EndTurn,
              stop_sequence: None,
              usage: Default::default(),
          })
      }
      async fn list_models(&self) -> Result<ListModelsResponse, ProviderError> {
          Ok(ListModelsResponse {
              models: vec![],
              default_model_id: None,
          })
      }
  }

  fn tagged_reg(id: &str, tag: &'static str) -> ProviderRegistration {
      ProviderRegistration::new(
          ProviderId::new(id).unwrap(),
          id,
          Arc::new(TaggedClient(tag)) as Arc<dyn ProviderClient + Send + Sync>,
          caps("m"),
      )
  }
  ```

  No test run yet — this step only adds shared fixtures.

- [ ] **Step 2: Write the failing tests first.** Append to `pool_lifecycle.rs`:

  ```rust
  #[tokio::test]
  async fn replace_provider_swaps_client_for_already_registered_id() {
      let mut cfg = HostConfig::new(
          ProviderEndpoint::StreamableHttp {
              url: "http://unused".into(),
          },
          "m",
      );
      cfg.providers = vec![tagged_reg("anthropic", "old")];
      cfg.startup_connect = StartupConnectPolicy::All;
      let host = Host::start(cfg).await.unwrap();

      let outcome = host.run_turn("hello").await.unwrap();
      assert_eq!(outcome.text, "old");

      host.replace_provider(tagged_reg("anthropic", "new"))
          .await
          .unwrap();

      // Still resolvable under the same id, active provider untouched.
      assert!(host.is_connected("anthropic").await);
      assert_eq!(host.active_provider().await.as_str(), "anthropic");
      assert!(host.active_capabilities().await.is_some());

      let outcome = host.run_turn("hello again").await.unwrap();
      assert_eq!(
          outcome.text, "new",
          "replace_provider must swap in the new client, not keep serving the old one"
      );
  }

  #[tokio::test]
  async fn replace_provider_adds_fresh_when_not_yet_registered() {
      let mut cfg = HostConfig::new(
          ProviderEndpoint::StreamableHttp {
              url: "http://unused".into(),
          },
          "m",
      );
      cfg.providers = vec![reg("anthropic", "m")];
      cfg.startup_connect = StartupConnectPolicy::All;
      let host = Host::start(cfg).await.unwrap();

      assert!(!host.is_connected("gemini").await);
      host.replace_provider(tagged_reg("gemini", "fresh"))
          .await
          .unwrap();
      assert!(host.is_connected("gemini").await);

      // Active provider (anthropic) is untouched by adding an unrelated one.
      assert_eq!(host.active_provider().await.as_str(), "anthropic");
  }

  /// Regression test for the check&#8594;remove TOCTOU race documented on
  /// `Host::replace_provider`'s doc comment: a concurrent removal landing
  /// between the initial `contains_key` check and the internal
  /// `remove_provider` call must resolve as a clean add, not a propagated
  /// `PoolError::NotRegistered`.
  #[tokio::test]
  async fn replace_provider_recovers_when_entry_removed_during_the_call() {
      let mut cfg = HostConfig::new(
          ProviderEndpoint::StreamableHttp {
              url: "http://unused".into(),
          },
          "m",
      );
      cfg.providers = vec![tagged_reg("anthropic", "old")];
      cfg.startup_connect = StartupConnectPolicy::All;
      let host = Arc::new(Host::start(cfg).await.unwrap());

      // Directly remove the entry to simulate a concurrent disconnect that
      // wins the race between replace_provider's check and its own removal
      // (both paths go through the same remove_provider primitive, so
      // calling it here before replace_provider runs is a faithful
      // simulation of "the entry is already gone by the time
      // replace_provider's internal remove_provider call runs").
      host.remove_provider(&ProviderId::new("anthropic").unwrap(), DisconnectMode::Force)
          .await
          .unwrap();
      assert!(!host.is_connected("anthropic").await);

      // replace_provider must still succeed — falling through to a clean
      // add — even though its (hypothetical) initial check would have
      // observed the entry present a moment earlier in a real race.
      host.replace_provider(tagged_reg("anthropic", "new"))
          .await
          .expect("replace_provider must recover from a NotRegistered race, not propagate it");
      assert!(host.is_connected("anthropic").await);
      let outcome = host.run_turn("hello").await.unwrap();
      assert_eq!(outcome.text, "new");
  }
  ```

  Run:
  ```bash
  cargo test -p otto-host replace_provider
  ```
  Expect **compile failure** (`Host::replace_provider` doesn't exist yet) — confirms the tests
  exercise the not-yet-built method.

- [ ] **Step 3: Implement `Host::replace_provider`.** In `crates/otto-host/src/session.rs`, directly
  below `add_provider` (after line ~2112, before the `remove_provider` doc comment), add:

  ```rust
  /// Replace an existing provider's pool entry with a freshly-built one, or
  /// add it fresh if it isn't registered yet. Used by `/connect`'s re-key
  /// flow: swaps in a client built from a newly-submitted credential for an
  /// already-connected provider, without requiring a manual `/disconnect`
  /// first. See `savvagent/otto#179`.
  ///
  /// The stale entry (if present) is removed with [`DisconnectMode::Force`]
  /// before the new one is inserted — not `Drain` — because this method is
  /// awaited inline from the TUI's synchronous key-event handling
  /// (`perform_connect`, never `tokio::spawn`ned the way `/disconnect`'s
  /// `remove_provider` call is); `Drain` would block the whole event loop for
  /// as long as any in-flight turn on the stale client takes to finish, which
  /// is unbounded. `Force` bounds the wait to
  /// `HostConfig::force_disconnect_grace_ms` (default 500ms) — a short,
  /// user-visible pause is an acceptable cost for a deliberate "I just typed a
  /// new key, use it now" action; hanging the UI is not. `active_provider`
  /// is untouched throughout: it stores only the `ProviderId`, which is
  /// reused, so once the new entry is inserted under the same id,
  /// `active_capabilities()` resolves again with no extra bookkeeping — same
  /// mechanism `perform_connect`'s existing drift-repair check already relies
  /// on.
  ///
  /// The initial `contains_key` check and the `remove_provider` call below
  /// are not atomic — the pool's read lock is released between them, so a
  /// concurrent removal (e.g. a `/disconnect` already in flight via its own
  /// `tokio::spawn`ned `remove_provider` call) can land in that window. If it
  /// does, this method's own `remove_provider` call observes the entry
  /// already gone and returns `PoolError::NotRegistered` — which is treated
  /// as "nothing to remove, proceed to add" rather than propagated, since
  /// that is exactly the outcome this method would have produced had the
  /// initial check observed `already_registered = false` to begin with. Any
  /// other error from `remove_provider` still propagates.
  pub async fn replace_provider(&self, reg: ProviderRegistration) -> Result<(), PoolError> {
      let id = reg.id.clone();
      let already_registered = self.pool.read().await.contains_key(&id);
      if already_registered {
          match self.remove_provider(&id, DisconnectMode::Force).await {
              Ok(()) | Err(PoolError::NotRegistered(_)) => {}
              Err(e) => return Err(e),
          }
      }
      self.add_provider(reg).await
  }
  ```

  Copy the doc comment and body verbatim from the spec's Approach §1 — they are the load-bearing
  race reasoning, not paraphrasable filler.

- [ ] **Step 4: Run the new tests.**
  ```bash
  cargo test -p otto-host replace_provider
  ```
  Expect all four PASS: `replace_provider_swaps_client_for_already_registered_id`,
  `replace_provider_adds_fresh_when_not_yet_registered`,
  `replace_provider_recovers_when_entry_removed_during_the_call`, plus the existing
  `add_provider_rejects_duplicate` (unaffected — confirms `add_provider` itself wasn't touched).

- [ ] **Step 5: Run the full `otto-host` test suite** to confirm nothing else regressed:
  ```bash
  cargo test -p otto-host
  ```
  Expect all PASS.

- [ ] **Step 6: Public-interface note.** `Host::replace_provider` is a new `pub async fn` — additive
  only. No SPP wire type, `ProviderHandler`/`ProviderClient` method, tool MCP schema, plugin ABI
  surface, slash command, env var, or on-disk transcript/keyring format is touched or renamed.
  `add_provider`/`remove_provider`/`PoolError`/`DisconnectMode` are all unchanged.

- [ ] **Step 7: Host-swap / streaming invariants — vacuously satisfied.** No
  `crates/otto/src/app.rs` or `crates/otto/src/tui.rs` code is touched by this task (that's Task 2,
  and even there only a call-site swap, no new `.await` under a lock). No streaming provider path
  (`provider-anthropic`/`provider-gemini`'s `ProgressDispatcher` usage) is touched.

- [ ] **Step 8: Format and commit.**
  ```bash
  cargo fmt --all
  git add crates/otto-host/src/session.rs crates/otto-host/tests/pool_lifecycle.rs
  git commit -m "otto-host: add replace_provider for re-keying an already-connected provider"
  ```

## Task 2: Point `perform_connect` at `replace_provider`; drop the dead locale key

**Files:**
- Modify: `crates/otto/src/main.rs`
- Modify: `crates/otto/locales/en.toml`
- Modify: `crates/otto/locales/es.toml`
- Modify: `crates/otto/locales/hi.toml`
- Modify: `crates/otto/locales/pt.toml`

- [ ] **Step 1: Confirm there is no existing direct unit test of `perform_connect`'s
  `AlreadyRegistered` branch to update.** (`perform_connect` is only exercised indirectly today,
  through the TUI event loop — there is no isolated unit test of this branch to rewrite, since it
  closes over `host_slot`/`app` state that isn't unit-testable in isolation, same reasoning the prior
  connect-area spec gave for `handle_api_key_modal_submit`.) Confirm by grepping:
  ```bash
  grep -n "fn.*perform_connect\|connect_already\|notes.connect-already" crates/otto/src/main.rs
  ```
  Task 1's `otto-host` tests are the regression coverage for this fix's actual logic; this task is a
  call-site swap with no new otto-side test needed, matching the spec's Goal & Success Criteria (all
  new tests live in `pool_lifecycle.rs`).

- [ ] **Step 2: Replace the `AlreadyRegistered`-handling branch.** In
  `crates/otto/src/main.rs`, find `perform_connect`'s "Pool already exists — add this provider to it
  additively" branch (currently around lines 2897-2916):
  ```rust
  } else {
      // Pool already exists — add this provider to it additively.
      let host = current_host(host_slot).await.expect("checked above");
      match host.add_provider(reg).await {
          Ok(()) => {}
          Err(otto_host::PoolError::AlreadyRegistered(_)) => {
              app.push_note(
                  rust_i18n::t!("notes.connect-already", name = spec.display_name).to_string(),
              );
              return;
          }
          Err(e) => {
              app.push_note(
                  rust_i18n::t!("notes.connect-failed", id = spec.id, err = format!("{e}"))
                      .to_string(),
              );
              return;
          }
      }
  }
  ```
  Change to:
  ```rust
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
  ```
  Do **not** touch anything downstream of this branch (the `active_is_in_pool` check,
  `should_promote`, `refresh_cached_models`, the `notes.connected-to` note, the
  `ProviderRegistered`/`Connect` dispatches) — all unchanged per the spec's Approach §2.

- [ ] **Step 3: Remove the now-dead `notes.connect-already` key.** In each of
  `crates/otto/locales/{en,es,hi,pt}.toml`, delete the `connect-already` line. Then confirm no
  reference remains anywhere:
  ```bash
  grep -rn "connect-already" crates/otto/
  ```
  Expect no output.

- [ ] **Step 4: Build and run the full `otto` test suite.**
  ```bash
  cargo build -p otto
  cargo test -p otto
  ```
  Expect a clean build and all PASS — including `crates/otto/tests/locales.rs`, which only checks
  the four locale files agree with each other (unaffected by removing the same key from all four).

- [ ] **Step 5: Public-interface note.** This changes `/connect`'s interactive behavior for an
  already-connected provider (re-keying now actually swaps the client instead of requiring a manual
  `/disconnect`). No SPP wire type, `ProviderHandler`/`ProviderClient` method, tool MCP schema,
  plugin ABI surface, slash command, env var, or on-disk transcript/keyring format is touched. Per
  the spec's "Public-interface changes" section, this ships as a `Fixed` (not `Changed`)
  `CHANGELOG.md` entry — it corrects behavior that contradicted the connect modal's own advertised
  "paste a new key" affordance, not a reversal of previously-intentional design.

- [ ] **Step 6: Host-swap / streaming invariants — vacuously satisfied.** No `.await` is added under
  any lock guard by this change; `perform_connect` already awaited the removed `add_provider` call
  the same way it now awaits `replace_provider`. No streaming provider path touched.

- [ ] **Step 7: Format and commit.**
  ```bash
  cargo fmt --all
  git add crates/otto/src/main.rs crates/otto/locales/en.toml crates/otto/locales/es.toml \
          crates/otto/locales/hi.toml crates/otto/locales/pt.toml
  git commit -m "otto: swap in a freshly-typed key for an already-connected provider on /connect"
  ```

## Task 3: Cut the release (notes only — performed per Phase 4 step 12, not in this PR)

**Files:** none in this PR.

- [ ] **Step 1:** This PR does **not** bump `workspace.package.version` and does **not** add a
  `CHANGELOG.md` section — that happens in the dedicated release PR after this merges, per
  Non-Negotiable Rule 8 / Phase 4 step 12. Re-read `workspace.package.version` and what's actually
  unreleased on `main` at cut time, and cut whatever line the batch actually requires (at least
  PATCH for this PR alone, per the "Release line" note at the top of this plan).
