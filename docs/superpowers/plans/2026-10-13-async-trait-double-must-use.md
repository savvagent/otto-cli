# async-trait double_must_use Implementation Plan

> Execute task-by-task (fresh implementer per task, review between tasks). Steps use checkbox
> (`- [ ]`) syntax for tracking.

**Goal:** Make the clippy lint gate clean on rustc 1.99 by upgrading `async-trait` to 0.1.92,
which no longer emits the redundant `#[must_use]`, and retire the #186 known-red entries.

**Architecture:** Dependency-only fix: raise the workspace `async-trait` requirement and
regenerate `Cargo.lock`. No Rust source changes. Then remove the #186 known-red bullets and prose
from `CLAUDE.md`'s Development Workflow section.

**Tech Stack:** Cargo workspace, `async-trait` 0.1.92 (pulls `syn` 3), clippy 1.99.0.

**Ticket:** savvagent/otto-cli#186

**Spec:** `docs/superpowers/specs/2026-10-13-async-trait-double-must-use-design.md`

## Global Constraints

- Gates (run from the worktree root):
  - format: `cargo fmt --all --check`
  - lint: `RUSTFLAGS="-D warnings" cargo +stable clippy --workspace --all-targets` on rustc
    1.99.0 (`rustc +stable --version`), and again with `+1.95.0`
  - extra: `RUSTFLAGS="-D warnings" cargo build --workspace --all-targets`
  - test: `RUSTFLAGS="-D warnings" cargo test --workspace --no-fail-fast`
  - when Cargo.toml: `dist plan`
- Evidence: report the `test result:` totals across all test binaries; a test binary that failed
  to build is not a pass. Skipped tests are not a pass.
- Invariants that apply: public interfaces change additively (none change); `otto-plugin` stays
  WIT-portable (its `Cargo.toml` is untouched); no `#[allow]` added to quiet the lint.
- Worktree only (`/home/robhicks/dev/otto-cli/.claude/worktrees/ci/double-must-use`); trunk
  changes only through the PR
- Commit format: `<scope>: <imperative, lower-case subject>` with a why-body; no attribution of
  any kind (no Co-Authored-By, no "Generated with", no credit markers)

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `Cargo.toml` | `[workspace.dependencies] async-trait = "0.1.92"` |
| **Modify.** `Cargo.lock` | `async-trait` 0.1.92, `syn` 3 added |
| **Modify.** `CLAUDE.md` | remove the #186 known-red entries and the "red on 1.99" prose |

## Task Order & Rationale

Task 1 fixes the lint (the gate is the failing test). Task 2 updates the workflow section, which
is only true once Task 1 lands. Task 3 runs the full gate set.

---

## Task 1: Upgrade async-trait

**Model:** haiku

**Files:** Modify `Cargo.toml`, `Cargo.lock`

- [ ] Step 1 (optional; already reproduced): Confirm the failure on main's lockfile:
  `git stash` any lockfile change, run
  `RUSTFLAGS="-D warnings" cargo +stable clippy -p otto-mcp -p otto-plugin --all-targets` and see
  `clippy::double_must_use` errors; `git stash pop`. (Already reproduced: 5 errors in `otto-mcp`,
  6 in `otto-plugin`.)
- [ ] Step 2: In `Cargo.toml` change `async-trait = "0.1"` to `async-trait = "0.1.92"`.
- [ ] Step 3: `cargo update -p async-trait` → `Cargo.lock` shows `async-trait` 0.1.92 and a new
  `syn` 3.x entry.
- [ ] Step 4: `RUSTFLAGS="-D warnings" cargo +stable clippy --workspace --all-targets` exits 0 on
  rustc 1.99.0. `grep -rn "double_must_use" crates` prints nothing.
- [ ] Step 5: Commit: `ci: upgrade async-trait to clear double_must_use on rustc 1.99`
  with a body explaining that 0.1.89 emitted a bare `#[must_use]` on the boxed-future method and
  0.1.92 does not. (Scope `ci`: the change is a workspace manifest fix for the CI lint job, not one crate.)

## Task 2: Retire the #186 known-red entries

**Model:** haiku

**Files:** Modify `CLAUDE.md`

- [ ] Step 1: Under `### Gates`, delete the `- known red: lint on rustc 1.99 …— #186` bullet.
- [ ] Step 2: Replace the `- toolchain:` bullet with:
  `- toolchain: no \`rust-toolchain.toml\`; CI floats \`stable\` (\`dtolnay/rust-toolchain@stable\`), so a local rustc can lag it and miss lints CI denies; MSRV is \`rust-version = "1.89"\` in the root \`Cargo.toml\``
- [ ] Step 3: Replace the prose line "The lint gate is clean on rustc 1.95 and red on 1.99 (#186);
  report which toolchain ran it (`rustc --version`)." with "Report which toolchain ran the lint
  gate (`rustc --version`); when it differs from CI's floating `stable`, rerun it on that version
  (`cargo +stable …` after `rustup update stable`)."
- [ ] Step 4: Under `### Ship`, delete the `- merge gate known red: fmt + clippy — #186 …` bullet.
- [ ] Step 5: `grep -n "#186\|known red" CLAUDE.md` prints nothing.
- [ ] Step 6: Commit: `docs: drop the #186 known-red lint entries from the workflow section`.

## Task 3: Full gate run

**Model:** haiku

- [ ] Run every gate in Global Constraints, including `dist plan` and lint on both toolchains;
  record commands, `test result:` totals, and HEAD. Verify-only: no commit.

## Task 4: Record as shipped (orchestrator, after the PR exists)

- [ ] Spec `Status:` becomes `IMPLEMENTED (#<pr>)`; tick this plan's boxes. Commit
  `docs: record async-trait double_must_use fix as implemented`. The PR body carries `Fixes #186`,
  the upgrade-vs-allow-vs-pin rationale, and "pre-merge checks: none apply".
