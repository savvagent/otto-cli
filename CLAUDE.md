# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Otto — a Rust-only, MCP-first terminal coding agent. Vision and scope live in `PRD.md`; the SPP wire format is in `crates/otto-protocol/SPEC.md`. Developer-facing details (slash commands, env vars, on-disk paths) are in `README.md`.

## Common commands

```bash
# Build everything. Required even for TUI-only work, because the TUI spawns
# `otto-tool-fs` at runtime and needs that binary to exist.
cargo build

# Run the TUI (default workspace member).
cargo run -p otto

# Tests.
cargo test --workspace
cargo test -p otto-host                     # single crate
cargo test -p otto-host -- name::of::test   # single test

# Continuous check / clippy via bacon.
bacon                # cargo check (default)
bacon clippy-all     # clippy across the workspace
bacon test           # cargo test

# Headless smoke-test (needs a provider — see README "Running providers as
# standalone MCP servers"):
cargo run -p otto-host --example headless -- "list my Cargo.toml"
```

`.env` and `.env.local` at repo root are auto-loaded on startup.

## Architecture (the parts you can't see by reading one file)

The core abstraction is "everything is MCP-shaped." A provider is just a `ProviderHandler` from `otto-mcp`; a tool presents an MCP tool surface, usually as a stdio server. Each can be wrapped in a binary (`provider-anthropic` ships `otto-anthropic`, `tool-fs` ships `otto-tool-fs`), but providers are linked **in-process by default** via `InProcessProviderClient` — the binary form exists for wire-protocol debugging.

### Turn loop

`Host` (in `otto-host`) holds a provider pool plus a `ToolRegistry`. `Host::run_turn_streaming` loops `provider.complete` → `tool_registry.call` until the model emits `end_turn`, forwarding `StreamEvent`s out as it goes. The tool-use loop, session state, and project-context loading (`OTTO.md` if present) all live here — the TUI is a thin shell on top.

### Host swap

The TUI keeps the active host in `HostSlot = Arc<RwLock<Option<Arc<Host>>>>` (`crates/otto/src/main.rs`). Per-turn worker tasks **clone the `Arc<Host>` under a brief read lock** (`current_host()`, same file) and then drop the guard before any `.await` — never hold the `RwLock` across awaits. The same holds for `Host`'s own `pool` and `active_provider` locks. `/connect` swaps the slot atomically. `tui.rs` is terminal init/restore only.

### Provider transport split

- **In-process (default):** `InProcessProviderClient` wraps a `ProviderHandler` directly — no HTTP, no serialization round-trip.
- **MCP HTTP (opt-in):** when `OTTO_PROVIDER_URL` is set, the TUI connects to a remote provider over `rmcp`'s Streamable HTTP transport instead.

The choice is per connection. `Host` keeps a provider pool (`pool: RwLock<HashMap<ProviderId, PoolEntry>>` plus an `active_provider` pointer, `crates/otto-host/src/session.rs`) that turns are routed to; `/connect` manages it through `add_provider`/`remove_provider`/`set_active_provider`. Provider selection goes through those APIs, never around them, and the host does not know which transport an entry uses.

### Tool transport

Bundled tools are stdio child processes owned by `ToolRegistry` and reaped on shutdown. Built-in plugins can also register an in-process handler with `Effect::RegisterInProcessTool`, which `ToolRegistry` looks up before the stdio children (`crates/otto-host/src/tools.rs`). All tool dispatch goes through `ToolRegistry`. User-configured `/mcp` entries can add either more stdio tool servers or remote Streamable HTTP MCP servers alongside them. The bundled tool binaries (`otto-tool-{fs,bash,grep,lsp,web}`) resolve through `$PATH` or an `OTTO_TOOL_<NAME>_BIN` override; additional built-in tools can be added via `HostConfig::with_tool` in `crates/otto/src/main.rs`.

### `rmcp` ProgressDispatcher gotcha

`subscriber.next()` from `rmcp`'s `ProgressDispatcher` does **not** auto-close when the RPC completes. Forwarder tasks that pump progress notifications must `JoinHandle::abort()` after the request future resolves, or the caller's mpsc waiter will deadlock. This pattern is used in `provider-anthropic`/`provider-gemini` streaming paths.

### TUI (`crates/otto`)

The main loop in `crates/otto/src/main.rs` polls crossterm (`event::poll` with a 50 ms timeout) and
redraws every iteration. It is not yet the `EventStream` + dirty-flag pattern; use that pattern for
new or refactored loop code, not as a description of what is there.

- Code that reads shared host or session state from the input loop goes through `current_host()`
  (Host swap above).
- `Effect` (`crates/otto-plugin/src/effect.rs`) is the vocabulary for plugin- and screen-driven side
  effects, and two dispatchers interpret it: `apply_canvas_effects` (`crates/otto/src/canvas_input.rs`)
  handles only `Effect::OpenUrl` and `Effect::Stack` for canvas-originated effects and logs the rest,
  while plugin-screen effects go through `crates/otto/src/plugin/effects.rs`. New interactive
  behavior adds an `Effect` variant wired into the dispatcher that owns its surface, rather than a
  screen reaching into `App` state.
- State transitions are unit-tested without a terminal (`ui::tests`, `canvas_input::tests`); keep
  new interactive logic testable the same way.
- Tests that spawn real subprocesses resolve the command through `PATH`, never a hardcoded absolute
  path (`/bin/true` is `/usr/bin/true` on macOS). See `open_url_system_browser_notes_success` and
  `_failure` in `crates/otto/src/canvas_input.rs`.

### Plugin ABI (native vs. WASM parity)

`otto-plugin` (native Rust trait surface), `otto-plugin-wit` (the WIT contract WASM guests bind
against), and `otto-plugin-wasm` (the host-side wasmtime adapter) are one interface — the plugin
ABI — a public interface (see Invariants under Development Workflow). Because `otto-plugin` is a plain
Rust crate, it can grow constructors and defaulted trait methods freely; `otto-plugin-wit` is a
WIT interface, where records can't carry associated functions and every new guest-callable
capability needs an explicit interface addition plus a regenerated guest binding. Left
unaddressed, the native surface drifts ahead of the WASM surface by default.

**Parity is the goal** for the plugin ABI's guest-callable behavioral surface (trait methods a
plugin implements to receive host callbacks — `Screen`, `Plugin`, hooks) and for constructor
ergonomics on WIT-backed value types wherever the guest side can express them in ordinary Rust
without a WIT change. Every `otto-plugin` addition to a surface already mirrored in
`otto-plugin-wit` should land its WIT/guest-ergonomics counterpart in the same change, or record
an issue number and the reason it doesn't (yet) — never a bare "for now" in `CHANGELOG.md`. A
capability family with no WIT interface at all gets named and tracked explicitly rather than left
to keep growing on the native side alone.

Current gap (surveyed 2026-09-11, tracked in `savvagent/otto-cli`):

| Native surface | otto-plugin-wit counterpart | Tracked |
|---|---|---|
| `Screen::ghost_completion` (`otto-plugin/src/screen.rs`) | none — `screen-instance`'s WIT resource has `on-key`/`on-event`/`render`/`tips` only | #165 |
| `StyledSpan`/`StyledLine` constructors (`otto-plugin/src/styled.rs`) | the WIT records exist; associated functions don't cross WIT at all | #166 |
| `ContentRenderer` (`otto-plugin/src/content.rs`) / `Plugin::create_renderer` (`otto-plugin/src/plugin.rs`) — canvas content blocks | none — no `plugin-canvas.wit` world exists | #167 |
| `Plugin::summarize_tool_call`/`summarize_tool_result` + `Contributions::tool_summaries` (`otto-plugin/src/plugin.rs`, `otto-plugin/src/manifest.rs`) | none — WIT `contributions` record has no `tool-summaries` field, no WIT function mirrors either method | #170 |

A future `CHANGELOG.md` entry that ships a native-only `otto-plugin` addition cites the tracking
issue for the WASM gap it opens, rather than "for now."

## Workspace map (for navigation)

| Crate | Owns |
|---|---|
| `crates/otto` | TUI binary, `/connect`, `/mcp`, file picker, transcript persistence, the `PROVIDERS` registry. |
| `crates/otto-host` | `Host`, `ToolRegistry`, session state, project context (`OTTO.md`). |
| `crates/otto-protocol` | Pure types: `CompleteRequest`, `CompleteResponse`, `StreamEvent`, content blocks. |
| `crates/otto-mcp` | `ProviderClient` / `ProviderHandler` traits and the `InProcessProviderClient` bridge. |
| `crates/provider-{anthropic,deepseek,gemini,grok,local,openai}` | Provider impls (libraries) + thin `otto-<vendor>` MCP-server binaries. |
| `crates/tool-{fs,bash,grep,lsp,web}` | Built-in tools as stdio MCP servers (`tool-fs`: `read_file` / `write_file` / `list_dir` / `glob`). |
| `crates/otto-canvas`, `crates/otto-fence` | Blitz-based HTML canvas renderer for inline content, and the streaming parser that extracts canvas fences from model text. |
| `crates/otto-plugin` | The `Plugin`/`Screen`/`ContentRenderer` traits, `Effect`, `StyledSpan`/`StyledLine`, and every other WIT-portable type a native plugin author builds against. |
| `crates/otto-plugin-wit` | The `.wit` contract (`plugin-static`/`plugin-interactive`/`plugin-provider` worlds, `shared.wit`) that WASM guests bind against. Dependency-light by design — see `crates/otto-plugin-wit/src/lib.rs`. |
| `crates/otto-plugin-wasm` | Host-side `wasmtime::component::bindgen!` adapters (`adapter/{static_,interactive,provider}.rs`) that load a `.wasm` component and present it as a `Box<dyn Plugin>`/`Box<dyn Screen>` to the rest of the host. |

## Extending

The README has step-by-step recipes — the short version:

- **New provider:** mirror `provider-gemini`, implement `ProviderHandler`, then append a `ProviderSpec` entry (and `build_*` factory) in `crates/otto/src/providers.rs::PROVIDERS`. The host needs no changes.
- **New tool:** mirror `crates/tool-fs` (stdio MCP server) and register it via `HostConfig::with_tool` in `crates/otto/src/main.rs`.
- **New user-configured MCP server:** use `/mcp` or add a `[[mcp_servers]]` entry in `~/.otto/config.toml`; secrets live in the keyring under `mcp:<server name>`.

## Persistence

- Transcripts: `~/.otto/transcripts/<unix>.json` (auto on `TurnComplete`, manual on `/save`).
- API keys and MCP-server secrets: OS keyring under service `otto`, account `<provider id>` or `mcp:<server name>`. Never written to disk in plaintext — `/connect` and `/mcp` are the only writers.


## Development Workflow

### Repo
- slug: savvagent/otto-cli
- trunk: main
- tracker: github
- ticketless work: open-issue
- status labels: in-progress, in-review

`savvagent/otto` redirects here (the root `Cargo.toml` `repository` and the README installer URL
still use it); pass `--repo savvagent/otto-cli` to `gh`.

### Tickets
- default assignee: none
- priority labels: none
- estimate labels: none
- types: bug=bug feature=enhancement docs=documentation epic=none

A multi-part effort's PRs say which part of the issue they ship (e.g. "E1 of #83"). The
housekeeping labels are the defaults (`wontfix`, `duplicate`, `invalid`); `question` is not
housekeeping.

### Branch, commit, PR
- branch: <area>/<kebab-slug>; area is a crate directory's short name (`host`, `protocol`, `mcp`, `plugin`, `canvas`, `tool-fs`, `provider-openai`, …) or a theme (`ci`, `docs`, `release`); release branches are `release/X-Y-Z` (e.g. `release/0-30-9`)
- commit: `<scope>: <imperative, lower-case subject>`; scope is the crate directory name (`otto`, `otto-host`, `otto-plugin`, `provider-grok`, `tool-fs`, …) or `docs`, `ci`, `release`; the PR title is the commit subject; squash-merged
- PR body must contain: Fixes #<issue> (a release PR lists every issue and PR it covers)
- merge: gh pr merge <pr> --repo savvagent/otto-cli --squash --delete-branch --admin
- after merge: git branch -D <branch>

The default-branch rulesets require one approving review, which a sole developer cannot give, so
PRs merge through the org-admin bypass (`--admin`). The rulesets require no status checks, so the
merge gate below is on the agent.

### Worktree
- path: .claude/worktrees/<branch> (a `/` in the branch nests: `.claude/worktrees/release/0-30-9`)
- bootstrap: `pkg-config --exists dbus-1 fontconfig` must succeed on Linux (keyring's secret-service backend and Blitz's fontconfig need the headers: `libdbus-1-dev libfontconfig1-dev pkg-config` on Debian/Ubuntu, `dbus-devel fontconfig-devel pkgconf` on Fedora); if it fails, report it rather than installing packages
- teardown: none

Tests are self-contained: no database, no network, no `.env` (which the TUI auto-loads at
startup but the gates do not need).

### Gates
- format: cargo fmt --all --check
- lint: RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets
- extra: RUSTFLAGS="-D warnings" cargo build --workspace --all-targets
- test: RUSTFLAGS="-D warnings" cargo test --workspace --no-fail-fast
- when Cargo.toml, .github/workflows/release.yml: dist plan
- when crates/otto-plugin-wit/Cargo.toml: `awk '/^\[dependencies\]/{f=1;next} /^\[/{f=0} f' crates/otto-plugin-wit/Cargo.toml | grep -E '^(ratatui|crossterm|tokio|anyhow|reqwest|wasmtime)([[:space:]]|=)'` must print nothing
- when crates/otto-plugin/Cargo.toml: `awk '/^\[dependencies\]$/{f=1;next} /^\[/{f=0} f' crates/otto-plugin/Cargo.toml | grep -E '^[[:space:]]*(ratatui|crossterm|tokio|anyhow)[[:space:]]*=' | grep -v 'wit-portable: justified'` must print nothing
- when examples/plugin-hello-*/**, crates/otto-plugin-wit/**: for d in examples/plugin-hello-*; do (cd "$d" && cargo component build --release --target wasm32-unknown-unknown) || exit 1; done
- evidence: report the `test result:` totals across all test binaries; a run where a test binary failed to build is not a pass. `#[ignore]`d live-vendor tests (`cargo test -p otto-host --test cross_vendor_history -- --ignored`, needs `*_API_KEY`s) are not part of the gate.
- toolchain: no `rust-toolchain.toml`; CI floats `stable` (`dtolnay/rust-toolchain@stable`), so a local rustc can lag it and miss lints CI denies; MSRV is `rust-version = "1.89"` in the root `Cargo.toml`

Report which toolchain ran the lint gate (`rustc --version`); when it differs from CI's floating `stable`, rerun it on that version (`cargo +stable …` after `rustup update stable`).
`RUSTFLAGS="-D warnings"` matches CI's workflow-wide env, and using it on every cargo gate keeps
them sharing one build cache. The `test` gate covers the offline cross-vendor matrix that CI's
`cross-vendor-gate` job reruns alone. The two `awk` gates are the `wit-dep-guard.yml` and
`wit-portability-guard.yml` checks. The plugin-example gate needs `cargo-component` and
`rustup target add wasm32-unknown-unknown`; CI runs it as a supplemental job. Code under
`cfg(windows)` or `cfg(target_os = "macos")` is compiled and tested only by CI's matrix: say so
in the PR body when the change touches it. CI's Windows leg excludes `otto-canvas` and `otto`
(Blitz font discovery hangs the test binary at startup on the hosted runner); that exclusion is
deliberate, not a regression.

### Design docs
- mode: committed
- spec path: docs/superpowers/specs/YYYY-MM-DD-<slug>-design.md
- plan path: docs/superpowers/plans/YYYY-MM-DD-<slug>.md
- shipped: final commit in the same PR before merge: the spec's `Status:` header line becomes `IMPLEMENTED (#<pr>)` and the plan's boxes are ticked. Files stay where they are; there is no archive directory.

Match the most recent spec and plan: the spec opens with `Date:`, `Status:`, `Source:` lines; the
plan with **Goal**, **Architecture**, **Tech Stack**, a `**Spec:**` line, a file map, then
`## Task N:` sections.

### Reviews
- language reviewers: crates/otto/src/** -> rust-pro (skill: rust:rust-tui), crates/otto-canvas/** -> rust-pro (skill: rust:rust-tui), crates/** -> rust-pro, examples/** -> rust-pro
- extra reviewers: none
- security focus: secrets (API keys and MCP secrets only in the OS keyring under service `otto`; never in a transcript, log line, error, or model-visible text); `tool-bash` command execution and `tool-web` network egress/SSRF; path traversal in `tool-fs`/`tool-grep`; plugin trust (native and WASM plugins gaining filesystem, network, or process reach beyond their declared capabilities; the WASM/WIT sandbox boundary); unsafe deserialization of provider and tool responses; MCP server configuration and OAuth (`/mcp`, `mcp_oauth.rs`); self-update downloading and running the release installer; unbounded reads, runaway turn loops, leaked child processes; anything that fails open
- automated reviewer: none

### Ship
- deploy: release
- merge gate: on the PR's head SHA: CI workflow `fmt + clippy`, `test (ubuntu-latest)`, `test (macos-latest)`, `test (windows-latest)`, `cross-vendor compatibility gate`, `dist plan`, and Release workflow `plan` succeeded; plus every other check that ran on the head SHA (`Forbidden dependency grep`, `forbid-non-portable-deps`)
- merge gate ignore: Example plugins build / build
- pre-merge checks: when crates/otto/src/main.rs, crates/otto/src/app.rs, crates/otto/src/tui.rs: TUI smoke: `cargo build` (the TUI spawns `otto-tool-fs` at runtime), then `cargo run -p otto` and exercise the changed surface (agent)
- pre-merge checks: when crates/provider-*/**: provider smoke: `cargo run -p otto-host --example headless -- "list my Cargo.toml"` with the provider's `*_API_KEY` set (agent)
- pre-merge checks: when crates/tool-*/**: tool smoke: the binary still resolves through `$PATH` or its `OTTO_TOOL_*_BIN` override, and its MCP input schema still validates (agent)
- pre-merge checks: when crates/otto-plugin*/**: plugin ABI smoke: build and load an `examples/plugin-hello-*` plugin (agent)
- pre-merge checks: when .github/workflows/**: workflow check: the changed jobs actually ran on the PR (agent)
- verify: the release is the deploy. SHA proxy: `git rev-parse v<version>^{commit}` equals the release PR's merge commit, and the Release workflow run for the tag (`gh run list -R savvagent/otto-cli --workflow Release --branch v<version>`) succeeded. Then `gh release view v<version> -R savvagent/otto-cli --json assets` lists all 14 assets: `dist-manifest.json`, `sha256.sum`, `source.tar.gz` and its `.sha256`, `otto-installer.sh`, `otto-installer.ps1`, and `otto-{aarch64-apple-darwin,aarch64-unknown-linux-gnu,x86_64-unknown-linux-gnu}.tar.gz` and `otto-x86_64-pc-windows-msvc.zip`, each with its `.sha256`; and the release is marked Latest. A missing asset means not released. For a feature PR, also confirm the CI run for its merge SHA (`gh run list -R savvagent/otto-cli --workflow CI --commit <merge-sha>`) succeeded.
- release: every merge to `main` is covered by a release before it counts as shipped, docs-only merges included. Cut it right after the merge, through its own `release/X-Y-Z` worktree and PR, by the manual process in `RELEASING.md` (release-plz is disabled until release-plz#2789 ships). The release folds in everything already merged and unreleased (`git log vLAST..origin/main`), takes the highest SemVer bump any of it needs (pre-1.0: MINOR for features and breaking changes, PATCH for fixes), gets a `CHANGELOG.md` entry per covered PR, and its PR body lists every covered issue and PR. Never hold a release for a future merge. The release PR is covered by the tag it creates. After the release PR merges, tag its merge commit `git tag -a v<version> -m "v<version>"` and `git push origin v<version>`; the tag triggers `release.yml` (cargo-dist). No crates.io publish (`release-plz.toml` sets `publish = false`); no `.deb`/`.rpm` or Homebrew packages.
- version files: Cargo.toml (`workspace.package.version` and every internal `workspace.dependencies` version), Cargo.lock (regenerate with `cargo check --workspace`), CHANGELOG.md

`Example plugins build` is `continue-on-error`: report a failure there in the PR body. Say in the
PR body which pre-merge checks ran; a TUI smoke with no interactive terminal, or a provider smoke
without the provider's key, is a hand-off to Rob.

### Out-of-band
- Release cut and tag push (agent): Ship > release.
- WASM test fixtures (agent, in the PR): a change under `crates/otto-plugin-wasm/tests/fixtures-src/` or `crates/otto-plugin-wit/wit/` rebuilds the committed `crates/otto-plugin-wasm/tests/fixtures/*.wasm` with `just build-fixtures`, and the rebuilt files are committed.
- New env var, slash command, or on-disk path (agent, in the PR): documented in `README.md`.

### Human-only
Hand these to Rob (robhicks) with the exact command; never run them:
- Repository settings, rulesets, and Actions secrets (e.g. `RELEASE_PLZ_TOKEN`).
- Deleting or moving a pushed tag or a published release.
- Re-enabling release-plz (`RELEASING.md` has the checklist).
- Anything that adds recurring cost (paid runners, a hosted service).

### Invariants
Reviewers check every one. The reasoning is under Architecture above.
- **Everything is MCP-shaped.** Providers are `ProviderHandler`s linked in-process by default; tools present an MCP surface (stdio server or in-process handler) and are dispatched only through `ToolRegistry`.
- **The turn loop lives in `Host`** (`otto-host`); `crates/otto` is a thin shell. `otto-protocol` holds pure types with no I/O; `otto-mcp` owns the provider traits.
- **Never hold a lock across `.await`:** `HostSlot`'s read guard, and `Host`'s `pool` and `active_provider` locks.
- **Provider selection goes through `Host`'s pool APIs.** No second registry, no hardcoded routing around the pool; transport (in-process vs. `OTTO_PROVIDER_URL`) is chosen per connection.
- **`rmcp` `ProgressDispatcher` forwarders are aborted** after the request future resolves, in every streaming path.
- **Secrets never touch disk in plaintext.** Keyring service `otto`, account `<provider id>` or `mcp:<server name>`; `/connect` and `/mcp` are the only writers.
- **Widening what `tool-bash`, `tool-web`, or a plugin can reach** (filesystem, network, process spawning) is a deliberate, reviewed change, never a refactor side effect.
- **Public interfaces change additively.** The SPP wire format (`crates/otto-protocol/SPEC.md`), the provider traits, tool names and input schemas, the plugin ABI, slash commands and env vars (`README.md`), and the transcript and keyring formats. A breaking change is named in the spec, the plan, `CHANGELOG.md`, and the PR body, and takes a MINOR bump.
- **Plugin ABI parity:** an `otto-plugin` addition lands its `otto-plugin-wit` counterpart in the same change or cites a tracking issue (Plugin ABI above).
- **`otto-plugin-wit` stays a leaf crate and `otto-plugin` stays WIT-portable** (the two `awk` gates).
- **Errors are actionable for an LLM caller:** no `unwrap()` outside tests, no silent fallback on a resolution failure.
