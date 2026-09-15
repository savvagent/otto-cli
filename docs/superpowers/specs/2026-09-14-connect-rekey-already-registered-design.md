# connect: re-keying an already-connected provider silently keeps the old client — design

Date: 2026-09-14
Status: approved
Source: savvagent/otto#179

## Problem

`/connect` → pick a provider that is already registered in the host's connection pool (already
connected in the current session) → the confirm-or-replace API-key modal opens (unconditionally,
since #178/v0.30.9) → type a new key and press Enter. The user reasonably expects the active
session to start using the new key immediately.

Instead, `perform_connect` (`crates/otto/src/main.rs:2780-3034`) persists the typed key to the OS
keyring via `creds::save` (so the credential IS updated on disk), builds a fresh
`ProviderRegistration` from it, then calls `host.add_provider(reg)`. `Host::add_provider`
(`crates/otto-host/src/session.rs:2091-2112`) refuses to insert over an existing pool entry and
returns `PoolError::AlreadyRegistered`. `perform_connect`'s handling of that error
(`crates/otto/src/main.rs:2902-2907`) discards the freshly-built client, pushes the
`notes.connect-already` note ("…is already connected. Run /disconnect %{name} first if you want to
connect again…"), and returns — the old `PoolEntry`, holding the stale client built from the old
key, stays active and keeps serving turns. A subsequent manual `/disconnect` + `/connect` does pick
up the new key correctly, since by then the keyring already has it, but nothing on the typed-key
submit path itself swaps the live client in.

`add_provider`'s refusal to silently clobber an existing entry is correct on its own terms — it's
the general-purpose "register a brand-new provider" API and a caller that hits `AlreadyRegistered`
by surprise (e.g. `apply_pending_pool_add`'s startup double-emit path, see "Out" below) genuinely
wants to no-op, not replace. The gap is specific to `perform_connect`'s modal-submit path: it has no
way to say "swap this pool entry for a freshly-built one," so it falls back to a manual-recovery
note.

## Approach

### 1. Add `Host::replace_provider` — an atomic swap-or-add pool API

`crates/otto-host/src/session.rs`, alongside `add_provider`/`remove_provider`:

```rust
/// Replace an existing provider's pool entry with a freshly-built one, or
/// add it fresh if it isn't registered yet. Used by `/connect`'s re-key
/// flow: swaps in a client built from a newly-submitted credential for an
/// already-connected provider, without requiring a manual `/disconnect`
/// first.
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
/// `active_capabilities()` resolves again with no extra bookkeeping —
/// same mechanism `perform_connect`'s existing drift-repair check already
/// relies on.
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
/// other error from `remove_provider` (only `Force`'s own internal paths
/// can produce one, and today none do) still propagates.
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

`PoolError`, `DisconnectMode`, and `add_provider`/`remove_provider` are unchanged — this is a new
method built entirely from the existing pool primitives, no new state, no new lock.

### 2. `perform_connect` calls `replace_provider` instead of `add_provider`

`crates/otto/src/main.rs:2897-2916`, the "pool already exists — add this provider to it
additively" branch changes from:

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

to:

```rust
} else {
    // Pool already exists — replace the entry if this provider is already
    // connected (re-key), or add it fresh otherwise. `replace_provider`
    // handles both: see savvagent/otto#179.
    let host = current_host(host_slot).await.expect("checked above");
    if let Err(e) = host.replace_provider(reg).await {
        app.push_note(
            rust_i18n::t!("notes.connect-failed", id = spec.id, err = format!("{e}")).to_string(),
        );
        return;
    }
}
```

The `PoolError::AlreadyRegistered` arm disappears: `replace_provider` only returns it if a *second*
registration for the same id races in between its own remove and add (the pool's write lock is
released between those two calls) — a genuine failure at that point, not the steady-state "you're
already connected" case this bug is about, so it falls through to the same generic
`notes.connect-failed` handling as any other pool error.

Everything downstream of this branch — the `active_is_in_pool` drift check, `should_promote`,
`refresh_cached_models`, the `notes.connected-to` note, the `ProviderRegistered`/`Connect`
`HostEvent` dispatches — is unchanged and untouched by this fix; it already runs unconditionally
after a successful add, and now runs identically after a successful replace. A re-key of the
currently-*active* provider (the common case) leaves `active_provider` pointing at the same id
throughout, so `active_is_in_pool` is true again by the time it's checked and `should_promote`
stays `false` — no redundant `set_active_provider`/`set_model` calls, matching today's behavior for
a second `/connect` where the active provider doesn't change. A re-key of a connected-but-inactive
provider behaves the same way: only that provider's pool entry is swapped, the active provider is
untouched.

### 3. `notes.connect-already` becomes dead; remove it

Its only call site is the branch removed in step 2. Remove the key from all four locale files
(`crates/otto/locales/{en,es,hi,pt}.toml`) — this repo's practice, per the most recent connect-area
spec, is to keep locale files free of unreferenced keys rather than leave a dangling translation.
`crates/otto/tests/locales.rs`'s structural-parity test only checks that the four locale files agree
with each other, not that every key is referenced, so removing the key from all four keeps that test
green; grep after editing to confirm no fourth reference exists.

## Scope

**In:**
- `crates/otto-host/src/session.rs` — new `Host::replace_provider` method.
- `crates/otto-host/tests/pool_lifecycle.rs` — new regression tests (see Goal & Success Criteria).
- `crates/otto/src/main.rs` — `perform_connect`'s `AlreadyRegistered`-handling branch replaced with a
  `replace_provider` call.
- `crates/otto/locales/{en,es,hi,pt}.toml` — `notes.connect-already` removed (now unreferenced).
- `CHANGELOG.md` — a `Fixed` entry (added in the dedicated release PR per Non-Negotiable Rule 8 /
  Phase 4 step 12, not in this PR).

**Out:**
- `Host::add_provider`, `Host::remove_provider`, `PoolError`, `DisconnectMode` — unchanged; this is
  additive (`replace_provider` is new, built on top).
- `apply_pending_pool_add`'s own `PoolError::AlreadyRegistered` handling
  (`crates/otto/src/main.rs:1964-1973`) — a different call site, for a genuinely different scenario
  (the startup double-emit dedup where `HostStarting`'s silent auto-connect and
  `Effect::RegisterProvider`'s drainer can both try to add the same provider; "already registered"
  there really does mean "nothing to do," not "the user wants this replaced"). Not touched by this
  fix; its existing no-op-and-return behavior stays exactly as is.
- `bootstrap_first_pool_host` — only reached on the `is_first_connect` branch, which this fix doesn't
  touch.
- The five keyed provider plugins' `handle_slash`, the connect picker screen, `enter_api_key_for`,
  `take_pending_api_key`, `handle_api_key_modal_submit` — all already correct (see the prior
  `2026-09-14-connect-stored-key-modal-design.md` spec); this fix starts from the same
  already-correct `(spec, api_key)` pair `perform_connect` has always received and changes only what
  happens once it discovers the provider is already in the pool.
- The host-swap `RwLock` discipline in `crates/otto/src/app.rs`/`tui.rs`, the provider transport
  split, `ToolRegistry` stdio plumbing, the `ProgressDispatcher` forwarder-abort pattern — none of
  this is touched.
- `README.md` — no documented `/connect` behavior changes; the modal already documents "press Enter
  to reuse, or paste a new key" and this fix makes the "paste a new key" half actually work when
  already connected, which is what that row already promises, not a new promise.

## Public-interface changes

**Additive only.** `Host::replace_provider` is a new `pub async fn` on `Host` — not a rename or
removal of any existing pool API (`add_provider`/`remove_provider`/`set_active_provider` are
unchanged), not a change to the SPP wire format, a tool's MCP schema, the plugin ABI, the
slash-command surface, an env var, or the on-disk transcript/keyring format. The **user-visible
behavior** changes (typing a new key for an already-connected provider now actually connects with
it, instead of requiring a manual `/disconnect` first) — this is the bug fix itself, classified as a
`Fixed` (not `Changed`) `CHANGELOG.md` entry: it corrects behavior that contradicts the connect
modal's own advertised "paste a new key" affordance, rather than changing a previously-intentional,
documented behavior (contrast with the prior connect-area spec's `Changed`-then-corrected-to-`Fixed`
entry, which reversed a *deliberately designed* silent-reconnect behavior from issue #82 — this bug
was never designed, just missed).

## Assumptions

- **`DisconnectMode::Force`, not `Drain`, is correct for `replace_provider`'s internal removal.**
  `perform_connect` is awaited directly from `run_app`'s synchronous key-event handling (every call
  site is `perform_connect(...).await` inline in a `match key.code`/`InputMode` arm), unlike
  `/disconnect`'s `remove_provider` call, which is explicitly `tokio::spawn`ned specifically so a
  long drain doesn't block TUI input (see that function's doc comment,
  `crates/otto/src/main.rs:2444-2453`). Using `Drain` here would reintroduce exactly the UI-freeze
  risk that comment describes, unbounded on how long the stale client's in-flight turn takes. `Force`
  bounds the wait to `force_disconnect_grace_ms` (500ms default) — a brief, bounded pause is an
  acceptable cost for a deliberate "use my new key now" submission; an unbounded one is not. A
  cooperative in-flight turn on the *old* client is cancelled (`HostError::Cancelled`), which is the
  expected outcome of deliberately replacing its credential mid-turn.
- **No change is needed to the active-provider promotion logic already in `perform_connect`.** Since
  `replace_provider` reuses the same `ProviderId` for the removed and re-added entry, and
  `active_provider` stores only the id (not a reference to the entry), the existing
  `active_is_in_pool`/`should_promote` drift-repair check already handles the brief remove→add
  window correctly with no new code — verified by tracing `active_capabilities()`'s pool lookup
  (`crates/otto-host/src/session.rs`, looks up `active_provider` by id in `pool`) against
  `replace_provider`'s sequencing.
- **`notes.connect-already` is fully dead after this fix and should be removed, not left orphaned.**
  Its only caller is the branch this fix rewrites; no other code path pushes it.
- **`apply_pending_pool_add`'s own `AlreadyRegistered` handling is out of scope**, because it's
  solving a different problem (startup dedup, not user-initiated re-key) with correct existing
  behavior — see "Scope: Out" above. This is a premise worth flagging explicitly since both call
  sites match on the same `PoolError` variant; only `perform_connect`'s is the one this issue is
  about.

## Goal & Success Criteria

Typing a new key for a provider that's already connected, and submitting it through the
confirm-or-replace modal, swaps the active session's client for that provider to one built from the
new key — without requiring a manual `/disconnect` first. Re-keying a provider that is not the
currently-active one leaves the active provider untouched. A provider genuinely not yet in the pool
still connects exactly as before (this path is additive, not a behavior change for first connects).

- `Host::replace_provider` exists, is exercised by new `#[tokio::test]`s in
  `crates/otto-host/tests/pool_lifecycle.rs` proving: (a) replacing an already-registered id swaps in
  the new client (a turn run afterward observes the new client's behavior, e.g. a distinguishable
  response/model id built into a test double, not the old one); (b) replacing an id that isn't yet
  registered behaves identically to `add_provider` (succeeds, pool gains the entry); (c) `id` stays
  resolvable via `is_connected`/`pool_snapshot` throughout, and `active_provider` (when it was
  pointing at the replaced id) still resolves via `active_capabilities()`/`active_provider()` after
  the swap with no manual repair; (d) the check→remove race — the entry is removed by a concurrent
  `remove_provider` call between `replace_provider`'s initial `contains_key` check and its own
  `remove_provider` call — resolves as a clean add rather than a propagated `NotRegistered` error.
  Simulate by spawning `replace_provider` in a `tokio::spawn`ned task, yielding once
  (`tokio::task::yield_now().await`) so the spawned task reaches its `contains_key` check and
  parks on the (now-suspended) `remove_provider` call, then calling `remove_provider` directly from
  the test to win the race, and asserting the spawned `replace_provider` still resolves `Ok(())`.
  (A *separate*, optional test may additionally cover the genuine-conflict race documented in Error
  Handling & Edge Cases §2 — two concurrent `replace_provider` calls for the same id — asserting
  exactly one succeeds and the other legitimately receives `PoolError::AlreadyRegistered`; do not
  fold that into case (d), whose assertion is "neither errors," which does not hold for that
  different race.)
- `perform_connect`'s `AlreadyRegistered`-specific note-and-return branch is gone; the only remaining
  error handling for the "provider already exists in the pool" branch is the generic
  `notes.connect-failed` path, reached only if `replace_provider` itself fails.
- `notes.connect-already` no longer appears in any of the four locale files or in
  `crates/otto/src/main.rs`.
- `cargo test -p otto-host` and `cargo test -p otto` both pass, including the new/updated tests.
- `cargo build --workspace --all-targets`, `cargo clippy --workspace --all-targets`, and
  `cargo fmt --all --check` are clean.

## Error Handling & Edge Cases

- **A concurrent removal lands between `replace_provider`'s initial `contains_key` check and its own
  `remove_provider` call** (e.g. a `/disconnect` already in flight via its own `tokio::spawn`ned
  `remove_provider` call, per the doc comment above). `remove_provider` then observes the entry
  already gone and returns `PoolError::NotRegistered`; `replace_provider` treats that specific
  outcome as "nothing to remove, proceed to add" rather than propagating it — see the doc comment on
  `replace_provider` in Approach §1. Without this handling, a user who disconnects and then
  immediately re-keys the same provider could see a spurious `notes.connect-failed` even though the
  correct, available outcome (clean add) was one branch away.
- **A second `add_provider`/`replace_provider` race lands between `replace_provider`'s internal
  `remove_provider` and `add_provider` calls.** `add_provider` returns `PoolError::AlreadyRegistered`
  again, which now propagates out of `replace_provider` as a genuine error; `perform_connect` surfaces
  it via the generic `notes.connect-failed` note. This is an existing, narrow, already-possible race
  class (the pool's write lock is released between the two calls inside `replace_provider`, same as
  it already is between any two independent pool calls) — not newly introduced, and not silently
  swallowed. Unlike the check→remove race above, this one is a genuine conflict (two callers both
  trying to *add* a fresh entry for the same id at the same moment) with no single correct winner, so
  it is surfaced rather than swallowed.
- **The stale client is mid-turn when the user re-keys it.** `DisconnectMode::Force`'s existing
  3-stage cancellation (cooperative cancel → bounded grace → hard abort) applies unchanged; the
  in-flight turn resolves as `HostError::Cancelled` (or, in the rare uncooperative case, is aborted
  after the grace period) exactly as an explicit `/disconnect --force` would.
- **Keyring write succeeds but the freshly-built client construction fails** (bad key format,
  `list_models` rejects it, etc.) — unchanged: `perform_connect` already returns before ever reaching
  the pool-replace branch in every one of those cases (see the existing `reg_result` match at
  `crates/otto/src/main.rs:2825-2869`), so a rejected new key never triggers a removal of the still
  valid old entry. The old client is only removed once the new one has already been built and
  validated.
- **Provider not yet in the pool (first-time connect for this provider, but the pool already has a
  different provider).** `replace_provider`'s `already_registered` check is `false`, so it's a plain
  `add_provider` call — identical to today's behavior on this branch.

## Risks & Open Questions

- **A brief (≤ ~500ms) pause is now possible on re-key if a turn is in flight on the old client**,
  where previously the operation returned instantly (with a note asking for a manual `/disconnect`).
  This is judged an acceptable, bounded trade-off — the alternative (today's behavior) is silently
  keeping the stale client active indefinitely, which is the bug this fixes.
- **No UI-level "reconnecting…" indicator is added for the brief force-disconnect window** — the
  existing `notes.connecting-to` note (pushed before the pool-replace branch runs, unchanged) already
  covers this; a dedicated spinner/progress indicator is out of scope for this fix and would be a
  separate UX enhancement if ever needed.
