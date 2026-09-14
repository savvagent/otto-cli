# Otto

A fast, MCP-first terminal coding agent written end-to-end in Rust.

The product vision and rationale live in [`PRD.md`](PRD.md). This README is
for developers working on the repo: how to build it, how it's laid out, and
how to extend it. End users wanting to install Otto can skip to
[Install](#install) below.

## Install

Precompiled binaries for Linux (x86_64 / aarch64), macOS (Apple Silicon),
and Windows (x86_64) are published to GitHub Releases on every tag. Each
release ships one archive per platform containing eleven binaries — the
`otto` TUI plus five bundled tool servers (`otto-tool-fs`,
`otto-tool-bash`, `otto-tool-grep`, `otto-tool-lsp`,
`otto-tool-web`) and five standalone provider MCP servers
(`otto-anthropic`, `otto-gemini`, `otto-openai`, `otto-deepseek`, `otto-grok`) —
installed to your Cargo bin directory. Local (Ollama) is linked into the
TUI and has no standalone shim.

**Linux / macOS** (one-liner):

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/savvagent/otto/releases/latest/download/otto-installer.sh | sh
```

**Windows** (PowerShell):

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/savvagent/otto/releases/latest/download/otto-installer.ps1 | iex"
```

**Manual:** download the matching `otto-<target>.tar.gz` (or `.zip`
on Windows) from the [Releases page](https://github.com/savvagent/otto/releases),
unpack it, and put the binaries on your `$PATH`. Each archive ships with a
`.sha256` next to it for verification.

After installing, run `otto` in your project, `/connect` once to store
an API key in the OS keyring, and you're done. The TUI checks for updates
on launch and installs them automatically in the background — all binaries
in the release archive are replaced in place, not just the main `otto`
executable. The banner above the prompt reports progress; restart otto
to use the new version. (`/update` becomes a retry/force-now command;
v0.11.0 through v0.12.1 only swapped the main binary and required a manual
installer re-run, so users on those versions must re-run the install script
once to land on v0.13.0 — auto-install takes over from there.)

## Repository layout

The workspace is a small set of focused crates:

| Crate | Purpose |
|---|---|
| [`crates/otto`](crates/otto) | All eleven shipping binaries (`otto` TUI plus the `otto-tool-{fs,bash,grep,lsp,web}` tool shims and the `otto-{anthropic,gemini,openai,deepseek,grok}` provider shims). Owns `/connect`, `/mcp`, file picker, transcript persistence, the plugin runtime. |
| [`crates/otto-host`](crates/otto-host) | Agent engine consumed as a library. Drives the tool-use loop, manages provider/tool sessions, owns the OS-level sandbox and per-tool stderr capture, exposes `Host::run_turn` and `run_turn_streaming`. |
| [`crates/otto-protocol`](crates/otto-protocol) | Pure-types crate: `CompleteRequest`, `CompleteResponse`, `StreamEvent`, content blocks, `ListModelsResponse`. SPP wire spec in [`SPEC.md`](crates/otto-protocol/SPEC.md). |
| [`crates/otto-mcp`](crates/otto-mcp) | The `ProviderClient` / `ProviderHandler` traits and the `InProcessProviderClient` bridge that makes provider crates linkable as libraries. |
| [`crates/otto-plugin`](crates/otto-plugin) | The `Plugin` / `Screen` traits and the `Effect` vocabulary every TUI feature (slash commands, modals, themes, language, providers) is expressed in. WIT-portable surface — see PRD §plugins. |
| [`crates/provider-anthropic`](crates/provider-anthropic) | Anthropic Messages API as a `ProviderHandler` library plus `provider_anthropic::run` (the entry point the `otto-anthropic` shim calls). |
| [`crates/provider-gemini`](crates/provider-gemini) | Google Gemini, same shape. Includes a JSON-Schema → OpenAPI-subset sanitizer for tool params. |
| [`crates/provider-openai`](crates/provider-openai) | OpenAI Chat Completions, same shape. |
| [`crates/provider-deepseek`](crates/provider-deepseek) | DeepSeek Chat Completions (OpenAI-compatible), same shape. |
| [`crates/provider-grok`](crates/provider-grok) | xAI Grok Chat Completions, same shape (OpenAI-compatible wire format). |
| [`crates/provider-local`](crates/provider-local) | Ollama (local) over its native HTTP API. Keyless; linked into the TUI only — no standalone shim. |
| [`crates/tool-fs`](crates/tool-fs) | `read_file` / `write_file` / `list_dir` / `glob` / `insert` / `replace` / `multi_edit` library plus `tool_fs::run` (the entry point the `otto-tool-fs` shim calls). |
| [`crates/tool-bash`](crates/tool-bash) | Sandboxed `bash` execution. The tool with the trickiest spawn lifecycle — see `otto-host::tools` for the lazy-spawn + `allow_net` resolver. |
| [`crates/tool-grep`](crates/tool-grep) | `ripgrep`-style search. |
| [`crates/tool-web`](crates/tool-web) | `web_fetch` (SSRF-guarded HTTP/HTTPS fetch with HTML-to-text conversion) and `web_search` (Brave Search API or self-hosted SearXNG backend). |

Every provider and every tool is "just" an MCP-shaped library that *can* be
wrapped in a binary. The TUI links providers in-process by default and
spawns the three tool servers (`tool-fs`, `tool-bash`, `tool-grep`) as
stdio children, optionally wrapped in an OS sandbox (`bwrap` on Linux,
`sandbox-exec` on macOS).

## Prerequisites

- Rust 1.85+ (workspace pins `rust-version = "1.85"`).
- Linux: a running freedesktop Secret Service for the keyring (GNOME Keyring,
  KeePassXC, or KWallet — any of them works). The crate falls back to a
  no-op when none is present, but `/connect` will fail to persist keys.
- macOS / Windows: nothing extra; the keyring uses the platform store.

## Quick start

```bash
# Build everything once. Important: the TUI doesn't depend on the tool-fs
# crate at compile time, but it spawns `otto-tool-fs` at runtime — a
# workspace build is the easy way to make that binary exist.
cargo build

# Run the TUI. With nothing configured, it boots disconnected.
cargo run -p otto
```

If the bundled tool servers (`otto-tool-fs`, `otto-tool-bash`,
`otto-tool-grep`) aren't on `$PATH` and aren't sitting next to the
TUI binary, the TUI still boots — affected tools are just disabled.
Re-run `cargo build` or set `OTTO_TOOL_{FS,BASH,GREP}_BIN` to point
at a specific path.

Inside the TUI:

1. Type `/` on an empty prompt to open the command palette. Keep typing
   to filter (e.g. `/co` narrows to
   `/connect`); <kbd>↑</kbd>/<kbd>↓</kbd> move, <kbd>Enter</kbd> selects,
   <kbd>Esc</kbd> cancels. You can also just type the full command
   (`/connect`) and press <kbd>Enter</kbd>.
2. Pick a provider with <kbd>↑</kbd>/<kbd>↓</kbd>, hit <kbd>Enter</kbd>.
3. Paste your API key (input is masked) and <kbd>Enter</kbd>.

The key is stashed in the OS keyring under service `otto`, account
`<provider id>`. On the next launch the TUI auto-connects to whichever
provider has a key on file.

### Other slash commands

| Command | What it does |
|---|---|
| `/connect` | Open the provider picker to add a provider to the connection pool. If the selected provider already has a stored key, the API-key modal opens with a "press Enter to reuse, or paste a new key" placeholder — press Enter on the empty field to keep using the stored key, or type a replacement to save and connect with a new one. Multiple providers can be connected simultaneously; switch with `/use <provider>`. |
| `/mcp` | Open the MCP-server manager. Lists configured user MCP servers, their transport (`stdio` or Streamable HTTP), HTTP auth mode (`none` / `bearer` / `oauth`), and whether startup connected them successfully. `a` adds a server, `d` removes one, `r` refreshes the status snapshot, `o` starts OAuth authorization for the selected OAuth HTTP server, and `c` checks whether the browser callback completed. Add/remove and successful OAuth authorization still require a restart to take effect. |
| `/disconnect <provider> [--force]` | Remove a provider from the pool. Default (drain) mode waits for any in-flight turn to finish. `--force` signals a cooperative cancel, waits 500 ms, then aborts. |
| `/use <provider>` | Switch the active provider. As of v0.17.0 the conversation history is preserved across the switch — `tool_use_id`s are provider-namespaced so subsequent turns on the new provider can safely see prior tool calls. For one-off routing without changing the active provider, use the `@<provider>` prefix. |
| `/model` | Open the model picker (no args), or switch directly: `/model gemini-2.5-pro`. As of v0.17.0 the picker lists every connected provider's models; selecting a model from a different provider switches the active provider too. Selection persists per provider to `~/.otto/models.toml`. |
| `/theme` | Open the theme picker (no args), or switch directly: `/theme tokyo-night`. Persists to `~/.otto/config.toml` under `[theme].name`. |
| `/language` | Open the locale picker. Persists to `~/.otto/config.toml` under `[language].code`. Ships with en / es / pt / hi; falls back to env detection, then en, when the saved value is missing or invalid. |
| `/plugins` | Open the plugin manager — toggle optional plugins on/off; core plugins can't be disabled. Persists to `~/.otto/plugins.toml`. |
| `/update` | Perform a live GitHub release check right now and install the newest release when one exists. The TUI also checks on launch and re-checks while it is open using `~/.otto/config.toml`'s `[update].periodic_interval_secs` (default 300 seconds); startup no longer trusts a fresh cache entry that only says the running version was current at the last check. The banner above the prompt reports progress and failures. Set `[update].disabled = true` to turn it off in config; `OTTO_NO_UPDATE_CHECK=1` or `--no-update-check` remain override switches for CI/scripting. Replaces every binary in the release archive — `otto` plus the six helpers. |
| `/save` | Write the current transcript to `~/.otto/transcripts/<unix>.json`. |
| `/save-canvas [path] [--block N] [--open]` | Write the most recent HTML canvas to a file. Default path is `otto-canvas-<id>.html` in the current directory. `--block N` targets a specific canvas by id; `--open` opens the file in the system browser after writing. |
| `/resume` | Re-open a previously-saved transcript and continue from where it ended. With no args opens a picker; takes an absolute path or a bare basename relative to `~/.otto/transcripts/`. |
| `/clear` | Reset the conversation history (and the visible log). |
| `/skills` | List every discovered skill (name, source tier, scope, and file-supplied description, marked as untrusted text), or `/skills <name>` to inject that skill's full instructions directly into the conversation. See "User-defined agents" below for the five discovery tiers and the level-3 trust gate. |
| `/tools` | List the tools registered with the current host, with their permission verdict. |
| `/bash <cmd>` | Run a shell command through `tool-bash`. `--net` / `--no-net` toggle network access for that single call. |
| `/sandbox` | Show or change OS-level sandbox settings; `/sandbox on` / `/sandbox off` persist to `~/.otto/sandbox.toml`. |
| `/prompt-keybindings` | Modal listing the keybindings active in the main prompt input. |
| `/exit` | Exit. |

`@` opens a file picker that inserts `@path` into the prompt.

## User-defined slash commands

Drop a markdown file under any of these directories and it becomes a slash command:

- `<project>/.otto/commands/` — project-local, preferred
- `<project>/.claude/commands/` — Claude Code compatible, project-local
- `~/.otto/commands/` — user-wide
- `~/.claude/commands/` — Claude Code compatible, user-wide

Project paths outrank user paths; within the same scope, `.otto/` outranks `.claude/`. Subdirectories become namespaces: `commands/team/lint.md` → `/team:lint`.

### Format

```markdown
---
description: Review the current diff
argument-hint: [commit range]
model: claude-sonnet-4-6
---

Please review the following diff and flag any issues:

!git diff $ARGUMENTS
```

| Token | Behavior |
|---|---|
| `$ARGUMENTS` | raw arg string |
| `$1`, `$2`, … | positional args |
| `@<path>` | inlined file contents (missing files leave the literal in place + warn) |
| `!<cmd>` | shell stdout; non-zero exit aborts dispatch |

### Trust prompt

The first time you invoke a project-local command that includes `!<cmd>`, Otto asks whether to trust the project. Decisions persist in `~/.otto/trusted-projects.json` (only "trust always" is stored).

### Reload

After editing a command file, run `/reload-commands` to rescan all four directories.

`/exit` is reserved by the built-in session-termination command, so a user-defined
`commands/exit.md` is ignored and logged as a warning during command discovery.

### `allowed-tools`

Parsed but not yet enforced; reserved for the upcoming agents sub-project.

## User-defined hooks

Drop a Claude-Code-compatible `settings.json` under any of these directories and the `hooks` block contributes shell hooks Otto fires at well-known event points:

- `<project>/.otto/settings.json`
- `<project>/.claude/settings.json`
- `~/.otto/settings.json`
- `~/.claude/settings.json`

Same precedence as user-defined slash commands (project beats user; within the same scope `.otto/` beats `.claude/`). All four files merge into one per-event index; `/reload-hooks` rescans without restart.

### Format

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "tool-fs:write_file",
        "hooks": [
          { "type": "command", "command": "scripts/audit-write.sh", "timeout": 5 }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "hooks": [
          { "command": "scripts/inject-context.sh" }
        ]
      }
    ]
  }
}
```

- `matcher` is a glob over the tool name (`*`, `tool-fs:*`, `tool-fs:write_file`); ignored for non-tool events.
- `timeout` is per-hook in seconds (default 60); the child is killed on timeout.
- `type` defaults to `"command"`; other values are reserved.
- Multiple `hooks[]` entries in one group run sequentially in declaration order.

### Events

| Event | Mapped to | Can block? |
|---|---|---|
| `PreToolUse` | before any tool dispatch | yes — `exit 2` or `{"continue":false}` cancels the tool call |
| `PostToolUse` | after a tool returns (observe-only, `*` matcher only in v1) | no |
| `UserPromptSubmit` | after the user submits a prompt, before the turn starts | yes — Block cancels the turn; `additionalContext` is prepended to the prompt |
| `SessionStart` | once at host startup | no |
| `Stop` | after the agent turn ends | yes — Block flags the turn as cancelled |

### Hook outcome protocol

Each hook is a shell command. Its stdin is JSON describing the event (`session_id`, `transcript_path`, `cwd`, `hook_event_name`, plus per-event fields like `tool_name` / `tool_input` / `prompt`). The outcome is determined by:

1. **Structured JSON stdout** — if the hook prints a JSON object with `{"continue": false, "stopReason": "…"}` or `{"hookSpecificOutput": {"additionalContext": "…"}}`, that takes precedence.
2. **Exit code 2** — non-empty stderr becomes the Block reason. For PreToolUse / UserPromptSubmit / Stop this blocks; for PostToolUse / SessionStart it's demoted to a warning since those events can't block.
3. **Any other exit code** — Continue. `suppressOutput` in stdout JSON silences the surfaced stdout/stderr PushNote.

`OTTO_PROJECT_DIR` is set in the child env so hooks can locate the project root regardless of `cwd`.

### Reload

Run `/reload-hooks` to rescan all four `settings.json` files without restarting the session.

## User-defined agents

Drop markdown files into any of these directories and Otto exposes them as subagents the model can spawn via the built-in `task` tool:

- `<project>/.otto/agents/**/*.md`
- `<project>/.claude/agents/**/*.md`
- `~/.otto/agents/**/*.md`
- `~/.claude/agents/**/*.md`

Same precedence as user-defined slash commands and hooks (project beats user; `.otto/` beats `.claude/`). First-wins dedup by filename slug. `/reload-agents` rescans without restarting the session.

### Format

```markdown
---
description: Reviews staged diffs for correctness bugs. Use after writing code, before commit.
tools: tool-fs:read_file, tool-fs:glob, tool-grep:search
model: claude-sonnet-4-6
---

You are a senior code reviewer. When invoked, ...
```

| Key | Required | Purpose |
|---|---|---|
| `description` | yes | Shown to the parent model as the `subagent_type` enum description (so the model knows when to pick this agent) |
| `tools` | no | Comma-separated string or YAML list of fully-qualified tool names. Omit to inherit the parent's full tool set. `[]` means only `task` is available. Defends against the model hallucinating tool names |
| `model` | no | Per-agent model override; falls back to the active model |
| `name` | no | Defaults to filename slug; warn-log if frontmatter disagrees |

Agent bodies may use `@<path>` on a line by itself to inline another file at load time (single-pass — included files containing `@<other>` are NOT recursively expanded). The body is the subagent's system prompt.

### The `task` tool

When at least one agent is discovered, Otto registers a built-in `task` tool whose `subagent_type` enum is populated from the discovered set. The parent model calls it with `{ description, prompt, subagent_type }`; the host spawns a Sub-Host (own session state, own model, filtered tool view) and returns the subagent's final assistant text as the tool result.

Subagent depth is capped by `OTTO_AGENT_MAX_DEPTH` (default 3); a subagent can spawn sub-subagents until that limit. PreToolUse user hooks (above) fire on subagent tool calls with an additional `subagent: "<name>"` field in the stdin payload. The `SubagentStop` event fires after each subagent's clean end-of-turn.

### Reload

Run `/reload-agents` to rescan all four `agents/` directories without restarting the session.

### Skills

Skills are a separate surface from these user-defined agents: instead of a
subagent the model spawns, a skill is a packaged set of instructions the
model *loads on demand*, following the Claude Code `SKILL.md` convention.
Run `/skills` to list the skills discovered across five tiers, highest
precedence first:

```
<project>/.otto/skills/<name>/SKILL.md
<project>/.claude/skills/<name>/SKILL.md
<project>/.github/skills/<name>/SKILL.md
~/.otto/skills/<name>/SKILL.md
~/.claude/skills/<name>/SKILL.md
```

Project beats user and `.otto/` beats `.claude/`; the first tier to claim a
slug wins, and a shadowed copy is reported so an author editing the losing
file finds out why nothing changed. That output reports each skill's name,
source tier, and file-supplied description, with the description labeled as
untrusted text; skills that fail to parse are skipped and reported as a
count.

`<project>/.github/skills/` is Copilot CLI's location rather than a Claude
Code one, and otto reads it so repos that already keep skills there — this
one included — work without moving them. It has no user-scope counterpart
because Copilot CLI only defines it inside a repository, and it ranks below
`.claude/` so a skill written in the format otto's own docs describe wins a
slug collision. For the trust gate it counts as project scope like the
other two: those skills arrive with the checkout.

`/reload-agents` only rescans the subagent directories above and does not
change the skills list — use `/reload-skills` below instead.

#### Skill format

A skill is a *directory* (not a single file, since it may bundle other
resources), holding a `SKILL.md`:

```markdown
---
name: rust-engineer
description: Use when building Rust systems where memory safety, ownership, and performance matter.
allowed-tools: tool-fs:read_file, tool-grep:search
---

# Rust engineer

You are an expert in idiomatic, safe Rust. When invoked, ...
```

| Key | Required | Purpose |
|---|---|---|
| `description` | yes | The only field shown to the model before it decides to load the skill; missing or blank is a hard parse error |
| `name` | no | Defaults to the directory slug; a frontmatter value that disagrees warns and the slug still wins |
| `allowed-tools` | no | Comma-separated string or YAML list. Parsed, but advisory only for now — see "Tool-name divergence" below |

Everything else in the directory — a `scripts/` subdirectory, `references/`,
`assets/`, or any other file — is a level-3 resource read on demand rather
than parsed frontmatter (see below).

#### Progressive disclosure

Skills load in three levels, so a skill's full size only costs tokens once
it's actually relevant:

1. **Level 1 — catalog.** Every discovered skill's `name` and `description`
   render into one system-prompt segment, so the model always knows what's
   available. Zero skills discovered means the segment is omitted entirely
   — there is no empty catalog costing tokens for nothing.
2. **Level 2 — body.** The rest of `SKILL.md` is returned only when the
   skill is actually loaded, via the `skill` tool or `/skills <name>` —
   never placed in the system prompt.
3. **Level 3 — bundled files.** A skill's `scripts/`, `references/`,
   `assets/`, or other sibling files are read on demand with
   `tool-fs`/`tool-bash`. The level-2 response always states the skill's
   root directory so relative paths in the instructions resolve.

#### The `skill` tool

Once at least one skill is discovered, otto registers a built-in `skill`
tool the model can call with `{ "name": "<skill>" }`; the `name` argument
is a live enum of currently-known skills, so a stale or mistyped name fails
schema validation before it reaches the handler. The tool returns the
skill's full body plus its root directory (level 2).

#### `/skills` and `/reload-skills`

- `/skills` with no argument lists every discovered skill: name, source
  tier, scope, and description (marked as untrusted text since it comes
  from a file otto did not author).
- `/skills <name>` injects that skill's full body directly into the
  conversation — the same payload the `skill` tool returns to the model,
  minus needing the model to ask for it first. An unknown name gets
  near-match suggestions rather than a bare "not found".
- `/reload-skills` rescans all five tiers, re-registers the `skill` tool
  against the refreshed set (so its name enum stays live), refreshes the
  level-1 catalog for the running session (a turn already in flight, or one
  that starts right after, would otherwise never see the change), and
  reports how many skills are now indexed.

#### Trust prompt

A `SKILL.md` file is inert markdown — level 1 and level 2 execute nothing.
Level 3 is different: a project-local skill whose directory bundles an
executable or a `scripts/` subdirectory is asking the model to run code
that arrived with the checkout, so the first time `/skills <name>` loads
such a skill, otto asks whether to trust the project — the same modal and
the same `~/.otto/trusted-projects.json` store user-defined commands use.
User-scope skills (`~/.otto/skills/`, `~/.claude/skills/`) never prompt;
they live in the user's own home directory rather than arriving with a
repo.

This prompt is reachable **only** through `/skills <name>`. The `skill`
tool has no interactive channel back to the user — when the model calls it
for a gated, not-yet-trusted skill, the tool can only refuse and tell the
model to point the user at `/skills <name>` instead; it can never pop the
modal itself.

Unlike user-defined commands, where "trust this session only" still blocks
`!shell` execution inside the expanded body, a skill has no equivalent
partial-trust mode: its body is either withheld entirely or released in
full, since there is no template-expansion step to intercept a bundled
script reference the way command expansion can. Choosing "session only" for
a skill therefore releases its complete body for the rest of the session —
functionally the same as "always trust", except the decision is not written
to `~/.otto/trusted-projects.json` and so does not carry over to the next
session.

#### Tool-name divergence

Claude Code's own commands, agents, and skills name Claude Code's tools
(`Read`, `Bash`, `Grep`, `Edit`); otto's model sees otto's own tool names
instead (`tool-fs:read_file`, `tool-bash:run`, `tool-grep:search`, …).
Three consequences:

- Prose inside a skill's body ("use the Read tool to check…") is left
  exactly as written — translating it automatically would risk corrupting
  instructions a capable model can already reinterpret against otto's
  actual tools.
- A frontmatter `allowed-tools` entry is parsed, but — like the
  `allowed-tools` caveat on user-defined commands above — is not yet
  enforced against the running tool registry; a name that doesn't match
  anything otto registers is neither an error nor filtered out today.
  Enforcing it, including dropping non-matching names with a load-time
  warning, is planned as part of command-parity follow-up work.
- A `[compat] tool_aliases` table mapping Claude Code tool names to otto's
  own is the obvious follow-up for translating `allowed-tools`
  automatically; it does not exist yet.

### Scrolling the conversation log

| Input | What it does |
|---|---|
| Mouse wheel | Three rows per notch. Up enters scrollback from the live tail; down snaps back to auto-tail on reaching the bottom. |
| <kbd>PageUp</kbd> / <kbd>PageDown</kbd> | Same offset, ten rows per press. |
| <kbd>Home</kbd> / <kbd>End</kbd> | Jump to the very top of history / back to auto-tail. (Requires <kbd>Ctrl</kbd> when the prompt has text — otherwise the keys go to the textarea.) |
| Submit a prompt, or <kbd>Esc</kbd> | Returns to auto-tail. |

Mouse capture is on, so terminal-native text selection no longer works
with a bare drag. Hold <kbd>Shift</kbd> while dragging to bypass capture
and select text the usual way.

### Routing turns to a specific provider/model

Prefix any message with `@<provider>:<model>` (or `@<provider>`, or
`@<alias>`) to route that single turn to a specific destination
regardless of the active provider:

- `@anthropic:claude-opus-4-7 design this` — explicit provider + model
- `@gemini explain this` — bare provider, picks Gemini's default model
- `@opus refactor this` — alias, resolves to Anthropic's claude-opus-4-7
- `@@team look here` — literal `@team` (strips one `@`)

Unknown `@`-tokens are not consumed: the message goes through verbatim
and the next turn routes to whichever provider `/use` last selected.
Each assistant turn shows a muted `▸ provider/model — Reason` line above
its response so the routing decision is always visible.

### Automatic modality routing

When you attach an image to your message, otto inspects the active
provider's chosen model. If it doesn't support vision (for example
`claude-haiku-4-5` or `o3`), the router automatically switches to a
sibling model on the **same provider** that does (e.g. haiku → opus on
Anthropic). The transcript badge above the response shows
`Modality(image)` when this happens.

If the active provider has no vision-capable model at all, the request
goes through to the active model unchanged and a muted note warns that
the model may reject it. The router does NOT silently jump to a
different provider — even if another connected provider has a
vision-capable model, that crosses a billing boundary you didn't pick.
Use `/use <provider>` to switch to a vision-capable provider, or
prefix the message with `@<provider>` to route just this turn.

Explicit `@provider:model` overrides always win, even when an image is
attached. If you pin a vision-incapable model with `@`, the request
still runs, the warning fires, and the provider's error (if any)
surfaces normally.

### Routing rules

Edit `~/.otto/routing.toml` to route turns to specific provider/model combinations based on the message. Example:

```toml
version = 1
default = "anthropic/claude-opus-4-7"

[[rule]]
name = "vision-for-images"
match = { has_image = true }
use = "gemini/gemini-2.0-flash-vision"

[[rule]]
name = "haiku-for-shortform"
match = { max_input_chars = 400 }
use = "anthropic/claude-haiku-4-5"
```

Rules evaluate top-to-bottom; the first match wins. Run `/route reload` after editing the file. Run `/route show` to see the active rules and the most recent routing decision. `@provider:model` overrides and modality redirects still take precedence over rules.

### Heuristic classifier (opt-in)

Add `heuristics = true` to `~/.otto/routing.toml` to turn on Layer 4 of the router — a hardcoded classifier that picks a cheaper or stronger model based on the shape of the user input:

- **Short question** (≤200 chars + a `?`) → cheapest connected model (`CostTier::Free` or `Cheap`).
- **Coding-flavored prompt** (contains any of `refactor`, `implement`, `debug`, `fix bug`, `compile`, `stack trace`, `function`, `class`, `error`) → strongest connected model (`Premium` or `Standard`).

The classifier prefers models on the **active provider** first, then walks the rest of the connected pool. If no connected model matches the desired tier — or the active model is already in that tier — the classifier yields nothing and the request falls through to your `/model` selection. Override (`@provider:model`), modality redirects (e.g. images → vision models), and explicit `[[rule]]` entries in `routing.toml` all beat the classifier when they apply.

**Caveats.** Coding keyword matching is **substring-based** in v1 — `function` matches `functional`, `error` matches `terror`. If you need stricter matching (whole-word only, custom keyword list, custom thresholds), use explicit `[[rule]]` entries instead; rules run earlier and beat the classifier.

Disable any time by setting `heuristics = false` (or removing the line) and running `/route reload`.

### Inline HTML rendering

Otto renders model-emitted HTML inline in the chat transcript when
your terminal supports an image protocol (Kitty / iTerm2 / WezTerm /
Ghostty / sixel). Models are prompted to wrap structured documents
(plans, specs, status updates, design reviews, comparison tables) in
` ```html-canvas ` fenced blocks; otto renders them as static
images in-transcript for now — Phase 2 will add mouse and keyboard
interaction.

Every rendered canvas is also auto-exported to
`~/.otto/canvases/<unix>-<turn>-<block>.html` so you can open it
in a real browser or share it. Use `/save-canvas [path] [--open]` to
write to an explicit location and optionally open it immediately.

Terminals without a supported image protocol show the HTML source in a
syntax-highlighted code block with a one-line banner; the content is
still fully readable.

Disable inline rendering by toggling the `internal:html-canvas` plugin
off in `~/.otto/plugins.toml`:

```toml
[plugins."internal:html-canvas"]
enabled = false
```

Auto-export is on whenever the plugin is enabled; there is no separate
toggle in v0.17.0. Disabling the plugin also stops auto-export.

See [`docs/canvas-terminal-compat.md`](docs/canvas-terminal-compat.md)
for the supported-terminal matrix and tmux passthrough setup.

### Multi-provider pool (Phase 1)

As of v0.15.0 Otto maintains a *connection pool* — you can `/connect`
multiple providers and they all hold active keyring sessions. The currently
active provider drives the turn loop; everything else sits connected but idle.

**Phase 1 invariant:** a conversation thread runs on one active provider
end-to-end. Switching providers with `/use <provider>` starts a fresh
conversation (history is cleared). Cross-provider routing within a single
conversation — auto-routing by cost/capability, `@provider:model` mid-turn
overrides — is deferred to Phase 3+ and depends on the cross-vendor
compatibility gate passing in Phase 2.

**Startup policy** is configured in `~/.otto/config.toml`:

```toml
[startup]
# Which providers to connect automatically when the TUI starts.
# "opt-in"   — only the providers listed in startup_providers (default)
# "all"      — every provider that has a key in the keyring
# "last-used"— reconnect to whichever provider was active at last exit
# "none"     — boot disconnected; use /connect manually
policy = "opt-in"
startup_providers = ["anthropic"]
connect_timeout_ms = 3000
# When false (default), startup connection problems (missing/rejected keys,
# build failures, timeouts) are logged but not shown as notes, so a normal
# launch stays quiet. Set to true to surface those notes at startup too —
# useful when diagnosing why a provider didn't come up automatically.
verbose = false

[migration]
# Set to true after the first-launch migration picker has run.
v1_done = true

[language]
code = "en"

[theme]
name = "dark"

[update]
periodic_interval_secs = 300
disabled = false
```

First-time users with multiple keys already in the keyring see a one-time
picker on launch that initializes `startup_providers`. Single-key users see
no UI change.

### Default behavior

Otto attaches a dynamic system prompt at the start of every
session, even when no `OTTO.md` is present and no override is
configured. The prompt covers:

- **Identity** — who Otto is and how it runs.
- **Behavior expectations** — use available tools proactively; don't
  claim a limitation without checking the tools first.
- **Tool affordances** — the *names* of the tools wired for this
  session (descriptions reach the model through the typed `tools`
  field, not via the system prompt). When the shell tool is wired,
  an explicit paragraph reminds the model that `gh`, `curl`, `git`,
  `rg`, package managers, and any installed CLI are reachable.
- **Environment** — OS, project root, git presence, Otto version.
- **Conventions** — `path/to/file.rs:42` link format; brief edit
  summaries.

To extend the prompt for your project, add a `OTTO.md` to the
project root. Its body is appended after the default and after any
embedder-supplied override, so your project guidance wins on
ambiguous points.

To suppress the default layer (embedders only):

```rust
let config = HostConfig::new(provider, model)
    .with_default_prompt_disabled();
```

This affects only the built-in default layer — the embedder
`system_prompt` override and `OTTO.md` body still compose if
present.

## Development workflow

```bash
# Continuous type-checking on save.
bacon                # default job is `cargo check`
bacon clippy-all     # clippy across the workspace

# Tests.
cargo test --workspace

# Specific crate.
cargo test -p otto-host

# The headless host smoke-test (needs a running provider — see below).
cargo run -p otto-host --example headless -- "list my Cargo.toml"
```

`bacon.toml` defines several jobs (`check`, `check-all`, `clippy`,
`clippy-all`, `test`, `doc`, `run`); pick whichever matches what you're
iterating on.

### Cross-vendor compatibility gate

`crates/otto-host/tests/cross_vendor_history.rs` exercises every
sender/receiver pair across the shipping providers to ensure foreign
`tool_use_id`s round-trip through each vendor's translator. PR CI runs
the offline (mocked) matrix as a dedicated `cross-vendor-gate` job. Live
variants are `#[ignore]`-marked; run them manually with the appropriate
`*_API_KEY` env vars:

```bash
ANTHROPIC_API_KEY=sk-… GEMINI_API_KEY=AIza… OPENAI_API_KEY=sk-… \
    cargo test -p otto-host --test cross_vendor_history -- --ignored
```

### Running the TUI in watch mode

There is no built-in watch mode for the TUI itself — bacon's `run` job
captures stdout, which doesn't play nicely with an interactive terminal
UI. For an actual restart-on-change loop, use `cargo-watch` in its own
terminal so the TUI gets a real TTY:

```bash
cargo install cargo-watch   # one-time
cargo watch -c -x 'run -p otto'
```

`tool-fs` is spawned at runtime, so make sure a workspace `cargo build`
has produced `otto-tool-fs`. If you want both steps explicit:

```bash
cargo watch -c -x build -x 'run -p otto'
```

For pure type-checking / clippy / test feedback while you edit, keep
`bacon` running in a separate pane.

### Running providers as standalone MCP servers

The default in-process path is the easy one. Sometimes you want the binary
form — e.g., when iterating on the wire format or running the `headless`
example. The standalone provider servers ship as bins on the `otto`
crate; each takes its API key via env and listens on loopback:

```bash
# Anthropic — defaults to 127.0.0.1:8787
ANTHROPIC_API_KEY=sk-ant-… cargo run -p otto --bin otto-anthropic

# Gemini — defaults to 127.0.0.1:8788
GEMINI_API_KEY=…           cargo run -p otto --bin otto-gemini

# OpenAI — defaults to 127.0.0.1:8789
OPENAI_API_KEY=…           cargo run -p otto --bin otto-openai

# DeepSeek — defaults to 127.0.0.1:8790
DEEPSEEK_API_KEY=…         cargo run -p otto --bin otto-deepseek

# xAI Grok — defaults to 127.0.0.1:8791
XAI_API_KEY=…              cargo run -p otto --bin otto-grok
```

Ollama (local) only runs as an in-process provider; there's no
`otto-local` shim because the upstream `ollama serve` already speaks
HTTP on `OLLAMA_HOST` (default `127.0.0.1:11434`).

Then point the TUI (or `otto-host` example) at it:

```bash
OTTO_PROVIDER_URL=http://127.0.0.1:8787/mcp cargo run -p otto
```

When `OTTO_PROVIDER_URL` is set the TUI uses the MCP client path
instead of the in-process bridge — useful for debugging the wire protocol
or pointing at a third-party MCP provider.

## Releases

Versioning and changelog generation are automated with
[release-plz](https://release-plz.dev) (`release-plz.toml`,
`.github/workflows/release-plz.yml`):

1. Every push to `main` runs `release-plz release-pr`, which opens or
   updates a `chore: release vX.Y.Z` pull request. It computes the next
   [SemVer](https://semver.org) bump and changelog entry from
   [Conventional Commits](https://www.conventionalcommits.org) merged
   since the last release — `fix:` → patch, `feat:` → minor, `!` or
   `BREAKING CHANGE:` footer → major.
2. Merging that PR (still subject to branch protection — one approval,
   or an admin bypass) updates `workspace.package.version` in `Cargo.toml`
   and `CHANGELOG.md` on `main`.
3. The same workflow then runs `release-plz release`, which tags the
   merge commit `vX.Y.Z` and creates a GitHub Release. That tag push is
   what triggers the existing `.github/workflows/release.yml` cargo-dist
   pipeline, building and publishing the installable binaries.

All workspace crates share one lockstep version, so a single release PR
covers the whole repo; the two test-fixture crates under
`tests/fixtures/` are excluded via `release-plz.toml`. Nothing is
published to crates.io. Use Conventional Commit prefixes (`feat:`,
`fix:`, `docs:`, `chore:`, etc.) in commit subjects so the automation
picks the correct version bump.

The workflow authenticates with a `RELEASE_PLZ_TOKEN` repo secret (a
PAT or GitHub App token) rather than the default `GITHUB_TOKEN`.
GitHub does not cascade further workflow runs from events triggered by
the default token, so using it would silently prevent both `ci.yml`
from running on the release PR and `release.yml` from firing when the
`vX.Y.Z` tag is pushed — defeating the point of the automation. The
token needs `contents: read/write` and `pull requests: read/write` on
this repository.

## Architecture in five sentences

`Host` (in `otto-host`) owns a `ToolRegistry` and a
*connected-provider pool* — a `HashMap<ProviderId, PoolEntry>` with one
entry marked active. Each `PoolEntry` wraps an `Arc<dyn ProviderClient>`
that is either an in-process bridge over a `ProviderHandler` (default) or an
`rmcp` Streamable HTTP client connected to a remote provider binary (opt-in).
Each user turn runs through `Host::run_turn_streaming`, which pulls the
active entry from the pool and loops `provider.complete` →
`tool_registry.call` until the model emits `end_turn`, forwarding stream
events to the TUI as it goes. The TUI keeps the host in
`Arc<RwLock<Option<Arc<Host>>>>` so per-turn tasks can snapshot it without
holding a lock across awaits, and `/connect` adds to the pool atomically.
Tool servers are stdio children, owned by the registry and reaped on
shutdown.

## Adding a new provider

1. Create `crates/provider-foo/` with the standard layout (mirror
   `provider-gemini`).
2. Implement `otto_mcp::ProviderHandler` for your `FooProvider` —
   translate to/from the upstream API, deal with streaming via
   `StreamEmitter`. The Anthropic and Gemini crates are the reference.
3. Expose a `FooProvider::builder().api_key(...).build()` constructor.
4. Append one entry to [`crates/otto/src/providers.rs::PROVIDERS`]:
   ```rust
   ProviderSpec {
       id: "foo",
       display_name: "Foo Models",
       api_key_env: "FOO_API_KEY",
       default_model: "foo-latest",
       api_key_required: true,
       build: build_foo,
       health_check: None,
   }
   ```
   …and a `build_foo` factory next to the existing five. Set
   `api_key_required: false` for keyless providers (the Local/Ollama
   entry is the reference); set `health_check: Some(check_foo)` if
   the provider has a reachability probe (also see Local/Ollama).
5. Wire `provider-foo` into `Cargo.toml` (workspace deps + otto crate
   deps).
6. Optional: ship a standalone `otto-foo` MCP server. Add a
   `pub async fn run()` to `provider_foo`'s lib (mirror `provider_anthropic::run`),
   then add a `[[bin]]` entry in `crates/otto/Cargo.toml` pointing at a
   3-line shim in `crates/otto/src/bin/otto-foo.rs` that just calls
   `provider_foo::run().await`. The release archive picks it up automatically.

That's the whole touch surface. There's no provider registry to update in
the host, no tool dispatch table — the host doesn't know about providers
beyond the `ProviderClient` trait.

## Adding a new tool

Tools are stdio MCP servers. Mirror `crates/tool-fs`:

1. Implement your tool methods using `rmcp`'s server primitives.
2. Build a binary that calls `serve_server` on stdin/stdout.
3. The host config takes a `ToolEndpoint::Stdio { command, args }` —
   `otto` currently bakes one in (`OTTO_TOOL_FS_BIN`); for
   additional tools you can extend `HostConfig::with_tool` calls in
   `crates/otto/src/main.rs`.

## Adding a new MCP server (user-configured)

For user-installed MCP servers, prefer config over rebuilding the binary.

1. Open `/mcp` inside otto and add either:
   - a local stdio server (`name`, `command`, optional `args`, optional one-secret env var), or
   - a remote Streamable HTTP server (`name`, `url`, auth mode `none` / `bearer` / `oauth`).
2. Otto persists the raw entry into `~/.otto/config.toml` under `[[mcp_servers]]`.
3. If a secret is provided, it is written to the OS keyring under service `otto`, account
   `mcp:<server name>` — never inline in TOML.
4. For `auth = "oauth"`, press `o` in `/mcp` to open the authorization URL in your browser, then
   press `c` after the provider redirects back to Otto's loopback callback listener. Otto stores
   the OAuth client registration, token set, and later refreshes in the same `mcp:<server name>`
   keyring slot as a structured JSON blob; bearer-token entries remain raw strings.
5. Remote HTTP MCP URLs must use `https://` unless they point at local loopback development
   endpoints (`127.0.0.1`, `localhost`, or `::1`).
6. Restart otto to apply the change. The `/mcp` screen shows whether startup connected the
   server or skipped it (for example, missing secret, authorization required, issuer mismatch, or
   token refresh failure).

The equivalent TOML looks like:

```toml
[[mcp_servers]]
name = "github"
transport = "stdio"
command = "/usr/local/bin/github-mcp-server"
args = ["--read-only"]
env = { GITHUB_TOKEN = "keyring" }

[[mcp_servers]]
name = "sentry"
transport = "http"
url = "https://mcp.sentry.dev/mcp"
auth = "bearer"

[[mcp_servers]]
name = "remote-oauth"
transport = "http"
url = "https://example.com/mcp"
auth = "oauth"
```

`env = { ... = "keyring" }` and `auth = "bearer"` mean "load the secret from the keyring account
`mcp:<server name>` during startup." All remote HTTP MCP URLs are fail-closed to HTTPS (loopback
`http://` is allowed only for local development). `auth = "oauth"` means "load the OAuth registration + token
state from the same keyring account, rediscover authorization metadata at startup, and connect with
an OAuth-aware HTTP client." OAuth startup skips are fail-closed: Otto refuses to reuse stored OAuth
state when the discovered issuer changes or the authorization server does not explicitly advertise
PKCE `S256` support.


### `read_resource` (built-in)

Always advertised. Takes `{ uri: string }` and returns the body of an
MCP resource. URIs surface in the conversation via
`[resource updated: <uri>]` user-text blocks the host injects at each
tool-use-loop iteration boundary; the model decides which (if any) to
pull. The host routes the read to whichever connected tool server
published the URI.

## Authoring external plugins

As of v0.18.0 Otto loads WASM Component-Model plugins from
well-known directories at startup. External plugins ship as a single
`.wasm` file plus a `plugin.toml` manifest, are trust-gated by SHA-256,
and adapt into the same in-process plugin registry the built-in
plugins live in — so anything the v0.9.0 `Plugin` trait can express
(slash commands, hooks, themes, render slots, keybindings, screens,
providers) can ship as a third-party plugin.

The long-form guide lives at [`docs/plugins/authoring.md`](docs/plugins/authoring.md);
the canonical contract is the design spec at
[`docs/superpowers/specs/2026-05-25-external-plugins-design.md`](docs/superpowers/specs/2026-05-25-external-plugins-design.md).

### Three worlds at a glance

A plugin declares exactly one WIT world; the world fixes what it can
contribute and which host capabilities it can import.

| World | Contributes | Host imports |
|---|---|---|
| `plugin-static` | slash commands, hooks, themes, render slots, keybindings | `log`, `current-theme` |
| `plugin-interactive` | screens (per-open state via Component Model resources) | `log`, `current-theme` |
| `plugin-provider` | a new provider in the `/connect` pool (`complete` / `list_models` / `count_tokens`) | `log`, `http` (allow-listed), `keyring` (allow-listed), `progress` |

One `.wasm` declares one world. Organizations that want to ship both
themes and a custom screen publish two plugins.

### Discovery paths

Mirror sub-projects A/B/C. First-wins by plugin id; project beats
user; `.otto/` beats `.claude/`:

1. `<project>/.otto/plugins/<id>/plugin.toml`
2. `<project>/.claude/plugins/<id>/plugin.toml`
3. `~/.otto/plugins/<id>/plugin.toml`
4. `~/.claude/plugins/<id>/plugin.toml`

Each plugin lives in its own directory named after its id. The
directory contains at least `plugin.toml` and (after install)
`plugin.wasm`.

### Minimal `plugin.toml`

```toml
[plugin]
id = "otto.hello-static"
name = "Hello (Static)"
version = "0.1.0"
world = "plugin-static"
otto = "^0.18"
description = "Minimal static-world example. Defines /hello."

[exports]
slash-commands = ["hello"]
```

`provider`-world plugins add a `provider-id` under `[exports]` and an
optional `[security]` block listing `allowed-hosts` (exact match in
v1) and `keyring-accounts`. See the
[authoring guide](docs/plugins/authoring.md#wit-contract-reference) for
the full schema and the [design spec §1](docs/superpowers/specs/2026-05-25-external-plugins-design.md)
for the canonical reference.

### `/plugins install <toml-url>`

The argument is a URL pointing at a remote `plugin.toml`. Otto
fetches and validates the manifest (64 KB cap), fetches the `wasm` URL
the manifest references (32 MB cap), computes a SHA-256 over the
whole staging tree, then opens a trust-prompt modal showing the
manifest fields, source URL, and tree hash. On confirm, the staging
directory is atomic-moved into `~/.otto/plugins/<id>/` and the
trust record is written to `~/.otto/plugin-trust.toml`. On
reject, the staging directory is deleted.

### `/plugins` subcommands

| Command | Behavior |
|---|---|
| `/plugins` | Open the plugin manager screen — lists every discovered plugin with trust status, world, exports, and source path. Toggle optional plugins on / off. |
| `/plugins install <toml-url>` | Install flow above. |
| `/plugins trust <id>` | Re-open the trust prompt for an untrusted plugin (e.g. after a tree-hash change). |
| `/plugins revoke <id>` | Drop the trust record. Plugin stays on disk, unloads next start. |
| `/plugins remove <id>` | Revoke + delete the plugin directory. |
| `/plugins enable <id>` | Clear `disabled-reason` (re-enable a plugin auto-disabled after repeated traps). |
| `/plugins disable <id>` | Set `disabled-reason = "manual"`, unload next start. |

Untrusted, hash-mismatched, or disabled plugins are skipped on load —
they appear in `/plugins` with the reason rendered as the row badge.

### Example plugins

Three runnable examples live under `examples/`:

- [`examples/plugin-hello-static/`](examples/plugin-hello-static/) — `/hello` slash command.
- [`examples/plugin-hello-interactive/`](examples/plugin-hello-interactive/) — opens a screen rendering `Hello, world!`.
- [`examples/plugin-hello-provider/`](examples/plugin-hello-provider/) — echo provider that returns the last user message.

Each example is intentionally outside the workspace (cargo-component's
profile setup clashes with the workspace release config). Build with:

```bash
cd examples/plugin-hello-static
cargo component build --release --target wasm32-unknown-unknown
```

The produced `.wasm` lands under that example's `target/`; drop it
plus the example's `plugin.toml` into `~/.otto/plugins/<id>/`
and trust it via `/plugins install` or `/plugins trust <id>`.

See [`docs/plugins/authoring.md`](docs/plugins/authoring.md) for the
full quickstart, capability matrix, three-strikes recovery, and the
explicit non-goals for v0.18.0.

## Language Server Protocol (LSP)

tool-lsp wraps user-configured LSP servers behind a small MCP tool surface (`lsp_definition`, `lsp_references`, `lsp_hover`, `lsp_document_symbols`, `lsp_workspace_symbols`, `lsp_rename`, `lsp_code_actions`) and publishes diagnostics as MCP resources via the `lsp://diagnostics/<absolute-path>` URI scheme. Diagnostics surface in the conversation as `[resource updated: lsp://diagnostics/<path>]` notes; the model fetches contents via the built-in `read_resource` tool.

### Quick start: `/lsp` installer

Run `/lsp` from inside otto to open a multi-select picker of curated language servers. Pick one or more with Space, confirm with Enter, and otto will:

1. download the pinned upstream binary (or run `npm i -g` for Node-based servers) into `~/.otto/lsp-bin/<server-id>/`,
2. verify the SHA256 checksum (binary entries only — npm trusts the registry),
3. merge the matching `[[language]]` entry into `~/.otto/lsp.toml`.

Restart otto after installing so `tool-lsp` re-reads the config.

The v1 catalog ships with `rust-analyzer`, `lua-language-server`, `typescript-language-server`, `pyright`, `bash-language-server`, and `vscode-langservers-extracted`. Node-based servers (the last four) require `npm` on `$PATH` — when it's missing, the picker still lists them but the install step prints an "install Node.js first" hint and skips that entry; the other selections in the batch still install.

### Configuration

Define which servers to launch in `~/.otto/lsp.toml` (global) and optionally override per-repo in `<repo>/.otto/lsp.toml`. There is no built-in default; tool-lsp is a no-op until you add at least one language. Repo-level entries fully replace global entries with the same `id` (no per-field merge).

```toml
[[language]]
id = "rust"
extensions = ["rs"]
root_markers = ["Cargo.toml", "rust-project.json"]
command = "rust-analyzer"
args = []
# Optional environment variables passed to the LSP child.
env = { RUST_LOG = "warn" }

[[language]]
id = "typescript"
extensions = ["ts", "tsx", "mts", "cts"]
root_markers = ["tsconfig.json", "package.json"]
command = "typescript-language-server"
args = ["--stdio"]

[[language]]
id = "python"
extensions = ["py"]
root_markers = ["pyproject.toml", "setup.py", "pyrightconfig.json"]
command = "pyright-langserver"
args = ["--stdio"]

[[language]]
id = "go"
extensions = ["go"]
root_markers = ["go.mod"]
command = "gopls"
args = []
```

### Notes

- The first call into a fresh project triggers `initialize` and (for rust-analyzer) full workspace indexing, which can take tens of seconds. tool-lsp emits MCP progress notifications during this wait so the TUI can show a status line.
- `lsp_rename` and `lsp_code_actions` return *edit descriptors* (`[{path, edits: [{range, new_text}]}]`) — they never apply changes themselves. The model issues `tool-fs::write_file` calls to apply them, keeping mutation inside the existing permission system.
- File rename/create/delete operations inside a `WorkspaceEdit` are rejected with an explanatory error; v1 supports plain in-file text edits only.
- Idle LSP sessions are evicted after ten minutes of no activity. Reopening a workspace triggers a fresh spawn + indexing pass.


## Environment variables

| Var | Where read | Default | Notes |
|---|---|---|---|
| `OTTO_PROVIDER_URL` | `otto` | (unset) | When set, skips in-process bridge; uses MCP HTTP. |
| `OTTO_MODEL` | `otto` | per-provider | Overrides both `~/.otto/models.toml` and `ProviderSpec::default_model`. Accepts `provider/model` form (`anthropic/claude-opus-4-7`) or bare model name (`claude-opus-4-7`; ambiguous bare names log a warning and fall back to the active provider). Precedence on connect: env > persisted > default. |
| `OTTO_TOOL_FS_BIN` | `otto` | `otto-tool-fs` (PATH) | Path to the fs tool binary. |
| `OTTO_TOOL_BASH_BIN` | `otto` | `otto-tool-bash` (PATH) | Path to the bash tool binary. |
| `OTTO_TOOL_GREP_BIN` | `otto` | `otto-tool-grep` (PATH) | Path to the grep tool binary. |
| `OTTO_TOOL_WEB_BIN` | `otto` | `otto-tool-web` (PATH) | Path to the web tool binary. |
| `OTTO_BRAVE_API_KEY` / `BRAVE_API_KEY` | `otto-tool-web` | (unset) | API key for `web_search` via the Brave Search API. |
| `OTTO_SEARXNG_URL` | `otto-tool-web` | (unset) | Base URL of a self-hosted SearXNG instance, used for `web_search` if no Brave key is set. |
| `OTTO_NO_UPDATE_CHECK` | `otto` | (unset) | Override switch for CI/scripting: when set, disables the launch-time and periodic update check regardless of `[update]` config, and disables `/update`. CLI equivalent: `--no-update-check`. |
| `ANTHROPIC_API_KEY` | `otto-anthropic` | — | Read at server start. In-process flow uses the keyring key from `/connect` when present, and falls back to this env var (used as-is, without prompting) when no keyring key is stored — including during automatic startup connection. |
| `ANTHROPIC_BASE_URL` | `otto-anthropic` | `https://api.anthropic.com` | For local mocks. |
| `OTTO_ANTHROPIC_LISTEN` | `otto-anthropic` | `127.0.0.1:8787` | Bind address. |
| `GEMINI_API_KEY` / `GOOGLE_API_KEY` | `otto-gemini` | — | Same idea as `ANTHROPIC_API_KEY`; `GEMINI_API_KEY` is checked first, then `GOOGLE_API_KEY`. |
| `GEMINI_BASE_URL` | `otto-gemini` | `https://generativelanguage.googleapis.com` | |
| `OTTO_GEMINI_LISTEN` | `otto-gemini` | `127.0.0.1:8788` | |
| `OPENAI_API_KEY` | `otto-openai` | — | Same idea as `ANTHROPIC_API_KEY`. |
| `OPENAI_BASE_URL` | `otto-openai` | `https://api.openai.com` | For local mocks. |
| `OTTO_OPENAI_LISTEN` | `otto-openai` | `127.0.0.1:8789` | Bind address. |
| `DEEPSEEK_API_KEY` | `otto-deepseek` | — | Same idea. |
| `DEEPSEEK_BASE_URL` | `otto-deepseek` | `https://api.deepseek.com` | For local mocks. |
| `OTTO_DEEPSEEK_LISTEN` | `otto-deepseek` | `127.0.0.1:8790` | Bind address. |
| `XAI_API_KEY` | `otto-grok` | — | Same idea. |
| `GROK_BASE_URL` | `otto-grok` | `https://api.x.ai/v1` | For local mocks. |
| `OTTO_GROK_LISTEN` | `otto-grok` | `127.0.0.1:8791` | Bind address. |
| `OLLAMA_HOST` | `otto` (Local provider) | `http://127.0.0.1:11434` | URL of the local Ollama HTTP server. |
| `RUST_LOG` | all binaries | `warn` (TUI), `warn` (tool servers) | Standard `tracing-subscriber` env filter. Tool servers' stderr is captured to `~/.otto/logs/tools/`, the TUI's tracing lands in `~/.otto/logs/otto.log`. |

`.env` and `.env.local` at the repo root are auto-loaded on startup.

## Persistence on disk

| Path | Owner | Contents |
|---|---|---|
| `~/.otto/transcripts/<unix_secs>.json` | TUI | One pretty-printed `Vec<spp::Message>` per save (auto on `TurnComplete`, manual on `/save`). |
| `~/.otto/canvases/<unix>-<turn>-<block>.html` | `internal:html-canvas` plugin | Auto-exported HTML source for each finalized canvas. Written with `0o600` permissions. Present only when the plugin is enabled. |
| `~/.otto/config.toml` | TUI startup, `/theme`, `/language`, `/mcp`, `internal:self-update` | Startup connection policy (`opt-in` / `all` / `last-used` / `none`), `startup_providers`, per-provider `connect_timeout_ms`, `[startup].verbose` (surface startup connection notes instead of just logging them; default `false`), one-time migration flag, `[language].code`, `[theme].name`, `[update]` settings (`periodic_interval_secs`, `disabled`), and `[[mcp_servers]]` entries for user-configured stdio/HTTP MCP servers. Created automatically on first launch when multiple keyring entries are found, and updated when those features persist settings. |
| `~/.otto/models.toml` | `/model` | `{ providers: { id = model } }`. Re-applied at `/connect`. |
| `~/.otto/plugins.toml` | `/plugins` | Optional plugin enabled-set. Core plugins ignore this file. |
| `~/.otto/sandbox.toml` | `/sandbox` | Sandbox mode + per-tool `allow_net` overrides. |
| `~/.otto/permissions.toml` | host | Per-tool / per-pattern permission verdicts. |
| `~/.otto/update-check.json` | `internal:self-update` | 24-hour cache of the GitHub Releases version probe. |
| `~/.otto/logs/otto.log` | TUI | All `tracing` output from the TUI process. |
| `~/.otto/logs/tools/<binary>.log` | host | Captured stderr from each spawned tool subprocess. |
| `~/.otto/trusted-projects.json` | user-defined slash commands | Project-trust persistence; only "trust always" decisions are stored. |
| `~/.otto/commands/` | user-defined slash commands | User-wide slash command markdown files. |
| `.otto/commands/` (per project) | user-defined slash commands | Project-local slash command markdown files. |
| OS keyring (`service=otto`, `account=<provider id>` or `mcp:<server name>`) | `/connect`, `/mcp` | Provider API keys, MCP-server bearer/env secrets, and OAuth client/token state for `auth = "oauth"` MCP servers. Never written to disk in plaintext. |

## Project context: `OTTO.md`

If a `OTTO.md` file exists at the project root the host reads it as
the system prompt. See `crates/otto-host/src/project.rs` and the
"Project context" section of the PRD.

## Reference docs

- [`PRD.md`](PRD.md) — vision, scope, milestones.
- [`crates/otto-protocol/SPEC.md`](crates/otto-protocol/SPEC.md) —
  Otto Provider Protocol (SPP) wire format.
- [`docs/`](docs) — architecture diagrams and design notes.

## License

Licensed under the GNU Affero General Public License v3.0 or later
(`AGPL-3.0-or-later`). See [`LICENSE`](LICENSE) for the full text.
