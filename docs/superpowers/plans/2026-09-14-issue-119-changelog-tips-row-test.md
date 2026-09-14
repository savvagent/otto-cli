# issue-119-changelog-tips-row-test Implementation Plan

**Goal:** Close out `savvagent/otto#119` by adding the regression test its acceptance criteria still
require — proof that `changelog/screen.rs`'s own last visible content row is never hidden by its
`tips()` row when painted through the real `paint_screen` path — and by recording, in the PR itself,
that the issue's other two acceptance criteria were already satisfied by PR #122 before this issue
was picked up. Fixes `savvagent/otto#119`.

**Fast-path:** no design spec per `otto-development`'s trivial-task criteria — this is a single-file
test addition with no interface change, no behavior change on any tested path, and a one-sentence
acceptance criterion ("prove `ChangelogScreen`'s last content row survives its own `tips()` row").
See the "Premise correction" note below for why the scope shrank from the issue's original three
bullets to one.

**Premise correction (recorded here since the spec step is skipped):** The issue was filed
2026-09-10T10:40:45Z, citing exact line numbers in `crates/otto/src/ui.rs` and
`crates/otto/src/plugin/builtin/{command_palette,changelog}/screen.rs` that still match current
`main`. But PR #122 (merged 2026-09-10T16:09:45Z, same day, fixing the closely related #116) already
implements the issue's first two acceptance-criteria bullets:

- **AC bullet 1** ("`paint_screen` shrinks the region it hands to `Screen::render` by one row
  whenever `tips()` is non-empty"): already true. `paint_screen`'s `Fullscreen` and `BottomSheet`
  arms (`crates/otto/src/ui.rs:1326`, `:1420`) both shrink the region before calling `render`, and
  `Screen::render`'s doc comment (`crates/otto-plugin/src/screen.rs:21-28`) now states this contract
  explicitly. `changelog`'s screen uses **`CenteredModal`** (`crates/otto/src/plugin/builtin/changelog/mod.rs:66-70`),
  a layout that was never affected by the overpaint bug in the first place — `paint_screen`'s
  `CenteredModal` arm renders `tips()` as the modal border's `title_bottom`
  (`crates/otto/src/ui.rs:1387-1393`), which lives on the border row outside `inner`
  (`inner = outer.inner(Margin { horizontal: 2, vertical: 1 })`, `ui.rs:1397-1400`), never inside the
  region handed to `render`. So `changelog/screen.rs:100`'s `let visible = region.height as usize;`
  — which the issue calls out as "no reservation for the tips row" — is already correct as written:
  there is nothing for it to reserve, because the region it receives never includes the tips row for
  this layout. The issue's steps-to-reproduce section is accurate about the code but incorrect about
  the consequence; PR #122's own body independently confirms this ("`CenteredModal` already got this
  right").
- **AC bullet 2** ("`command_palette/screen.rs`'s now-redundant hardcoded `.saturating_sub(2)` budget
  is removed or reduced"): already done. `capacity` is now `(region.height as
  usize).saturating_sub(1).max(1)` (`crates/otto/src/plugin/builtin/command_palette/screen.rs:161`),
  reserving only the palette's own spacer/scroll-hint row — the tips row is no longer double-counted.

**What is actually missing — AC bullet 3:** "A regression test proves `changelog`'s last visible row
is never overpainted by its own tips row, mirroring
`cursor_row_never_lands_on_the_tips_row_at_any_height` in `command_palette/screen.rs`)." No such test
exists today. `crates/otto/src/ui.rs` has a generic mechanism-level test
(`paint_screen_reserves_tips_row_out_of_the_region_before_render`) using a synthetic
`RegionRecordingScreen`, and the command palette has its own concrete-screen regression
(`palette_selected_row_paints_above_the_tips_row_when_scrolled`, the renamed successor to the test
the issue names) — but nothing exercises the real `ChangelogScreen` end to end through `paint_screen`
to confirm its `CenteredModal` rendering is what it appears to be. This plan adds exactly that test,
matching the existing `palette_selected_row_paints_above_the_tips_row_when_scrolled` pattern: paint
the real screen through the real `paint_screen`, inspect the resulting `Buffer`'s text, and assert the
last content line and the tips line both land on separate, visible rows.

**Architecture:** No production code changes. One new `#[tokio::test]` in
`crates/otto/src/ui.rs`'s existing `mod tests` block, using the same `render_paint_screen` /
`buffer_text` test harness already used for the command-palette regression tests, applied to
`crate::plugin::builtin::changelog::screen::ChangelogScreen` with
`ChangelogState::Loaded { lines }` seeded with more lines than the `CenteredModal`'s inner height at
the harness's fixed 100x30 `TestBackend`, so the tail of the content is exercised exactly like the
palette's height-sweep test. No crate boundary changes — `crates/otto` already owns both `ui.rs` and
the `changelog` screen module (Load-Bearing Invariant 2 doesn't apply; this isn't the turn loop). No
host-swap `RwLock` code touched. No streaming provider path touched.

**Tech Stack:** Existing workspace only (`ratatui::backend::TestBackend`, `rust_i18n`, `std::sync::{Arc,
Mutex}` — all already imported/used elsewhere in this test module or the `changelog` module). No new
dependencies.

**Release line:** at least PATCH after whatever `workspace.package.version` reads at cut time
(currently `0.30.7` as of branch creation — re-read at cut time per Phase 4 step 12, since
`origin/main` moves while this branch is open). Test-only change, no public-interface change → PATCH
per `CHANGELOG.md`'s convention (this could ship under a higher batched line if other unreleased work
lands first — see Non-Negotiable Rule 8).

**Branch:** `otto/changelog-tips-row-test`

## File Map

**New files**
- `docs/superpowers/plans/2026-09-14-issue-119-changelog-tips-row-test.md` — this plan (no design
  spec — fast-path).

**Modified files**
- `crates/otto/src/ui.rs` — one new regression test in `mod tests`.
- `CHANGELOG.md` — a `Fixed`/test-coverage entry (added in the dedicated release PR per Phase 4 step
  12, not in this PR).

## Task 1: Add the changelog-screen tips-row regression test

**Files:**
- Modify: `crates/otto/src/ui.rs`

- [ ] **Step 1: Write the failing test first.** In `crates/otto/src/ui.rs`'s `mod tests` block, near
  `palette_selected_row_paints_above_the_tips_row_when_scrolled` (currently ends around line 1859),
  add:

  ```rust
  /// The `changelog` screen's own regression for the tips-overpaint contract:
  /// paint the real `ChangelogScreen` (not a synthetic stand-in) through the
  /// real `paint_screen`, in its actual `CenteredModal` layout, and confirm
  /// its last visible content row survives alongside the tips row rather
  /// than being hidden by it. `changelog` uses `CenteredModal`, where
  /// `tips()` paints as the modal border's bottom title
  /// (`paint_screen`'s `CenteredModal` arm) rather than by shrinking the
  /// content region — a structurally different mechanism than the
  /// `Fullscreen`/`BottomSheet` reservation the command-palette tests cover,
  /// so it needs its own concrete-screen proof. Mirrors
  /// `palette_selected_row_paints_above_the_tips_row_when_scrolled`.
  #[tokio::test]
  async fn changelog_last_content_row_survives_its_own_tips_row() {
      use crate::plugin::builtin::changelog::screen::{ChangelogScreen, ChangelogState};
      use std::sync::{Arc, Mutex};

      // More lines than the CenteredModal's inner height can show at once
      // (90%/85% of the harness's fixed 100x30 backend), so the tail of the
      // content is exercised — the same "overflow, then check the last
      // visible row" shape as the palette's height-sweep test.
      let lines: Vec<StyledLine> = (0..40)
          .map(|i| StyledLine::plain(format!("changelog-line-{i:02}")))
          .collect();
      let screen = ChangelogScreen::new(Arc::new(Mutex::new(ChangelogState::Loaded {
          lines: lines.clone(),
      })));

      // Derive the tips needle from the screen itself, like the palette
      // test does, so a locale switch elsewhere in this test binary can't
      // turn this into a flake.
      let tips_text: String = screen.tips()[0]
          .spans
          .iter()
          .map(|s| s.text.clone())
          .collect();
      let tips_needle = tips_text
          .split(' ')
          .next()
          .expect("tips line is non-empty")
          .to_string();

      let buffer = render_paint_screen(
          &screen,
          &ScreenLayout::CenteredModal {
              width_pct: 90,
              height_pct: 85,
              title: Some("Changelog".to_string()),
          },
          Palette::for_theme(Theme::Dark),
          crate::splash::SandboxSplashState::OnDefault,
      );
      let text = buffer_text(&buffer);

      // The harness's TestBackend is 100x30: height_pct 85 -> outer height
      // 25, minus the 1-row top/bottom border margin -> inner height 23.
      // With 40 lines and no scroll, the visible window is lines[0..23], so
      // "changelog-line-22" is the last content row this screen draws.
      let last_visible = "changelog-line-22";
      assert!(
          text.contains(last_visible),
          "the changelog screen's own last visible content row must survive \
           painting, not be hidden by the tips row:\n{text}"
      );

      let content_row = text
          .lines()
          .position(|l| l.contains(last_visible))
          .unwrap_or_else(|| panic!("last content row must paint:\n{text}"));
      let tips_row = text
          .lines()
          .position(|l| l.contains(&tips_needle))
          .unwrap_or_else(|| panic!("the tips row must paint:\n{text}"));
      assert!(
          content_row < tips_row,
          "the changelog screen's last content row {content_row} must paint \
           above the tips row {tips_row} (a modal border row, never inside \
           the content region):\n{text}"
      );

      // A line past the visible window must not appear anywhere — confirms
      // the assertion above is actually exercising a clipped/overflowing
      // buffer, not a coincidentally short one.
      assert!(
          !text.contains("changelog-line-23"),
          "a line past the visible window must not paint:\n{text}"
      );
  }
  ```

- [ ] **Step 2: Confirm the test compiles and passes.**
  ```bash
  cargo test -p otto changelog_last_content_row_survives_its_own_tips_row
  ```
  Expect: PASS. If the computed inner height differs from 23 (e.g. a `ratatui` version bump changes
  rounding), the assertion on `"changelog-line-22"` will fail with the full buffer text printed —
  adjust the expected last-line index to match `render_paint_screen`'s actual `TestBackend` geometry
  rather than the terminal's real size, and update the comment's arithmetic to match.

- [ ] **Step 3: Confirm this test actually guards something (not a vacuous pass).** Temporarily edit
  the test's `ScreenLayout::CenteredModal` height_pct to `100` (or otherwise make `inner` cover all 40
  lines) and rerun — the "line past the visible window must not appear" assertion should now fail,
  proving the original parameters really do exercise a clipped view. Revert this temporary edit before
  continuing (do not commit it).

- [ ] **Step 4: Run the full `otto` crate test suite.**
  ```bash
  cargo test -p otto
  ```
  Expect: all tests pass, including the new one and the pre-existing
  `paint_screen_reserves_tips_row_out_of_the_region_before_render` and
  `palette_selected_row_paints_above_the_tips_row_when_scrolled`.

- [ ] **Step 5: Full workspace build and lint.**
  ```bash
  cargo build --workspace --all-targets
  cargo clippy --workspace --all-targets
  cargo fmt --all --check
  ```
  Expect all clean (CI runs clippy with `RUSTFLAGS=-D warnings`).

- [ ] **Step 6: Public-interface note.** No SPP wire type, `ProviderHandler`/`ProviderClient` method,
  tool MCP schema, plugin ABI surface, slash command, env var, or on-disk transcript/keyring format
  touched — Non-Negotiable Rule 6 is not engaged. This is a test-only addition to a `#[cfg(test)]`
  module.

- [ ] **Step 7: Host-swap / streaming invariants — vacuously satisfied.** No `crates/otto/src/app.rs`
  or `crates/otto/src/tui.rs` touched, so the host-swap `RwLock` rule is not engaged. No streaming
  provider path touched, so the `ProgressDispatcher` forwarder-abort pattern is not engaged.

- [ ] **Step 8: Format and commit.**
  ```bash
  cargo fmt --all
  git add crates/otto/src/ui.rs
  git commit -m "otto: add changelog screen tips-row overpaint regression test"
  ```

## Task 2: Cut the release (notes only — performed per Phase 4 step 12, not in this PR)

**Files:** none in this PR.

- [ ] **Step 1:** This PR does **not** bump `workspace.package.version` and does **not** add a
  `CHANGELOG.md` section — that happens in the dedicated release PR after this merges, per
  Non-Negotiable Rule 8 / Phase 4 step 12. Re-read `workspace.package.version` at cut time (it may
  have moved past `0.30.7` if another PR merges first) and cut the next PATCH (or higher, if batched
  with other unreleased work) from whatever it then reads. The `CHANGELOG.md` entry: something like
  "Added a regression test proving the `changelog` screen's own content survives its `tips()` row
  when painted through its `CenteredModal` layout (`savvagent/otto#119`)."
