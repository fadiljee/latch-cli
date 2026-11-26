<p align="center">
  <img src="assets/icon.png" alt="Latch Logo" width="128" />
</p>

<h1 align="center">Latch CLI</h1>

<p align="center">
  <strong>Autonomous Rust Agent Runtime CLI</strong> — run any AI coding agent in parallel isolated <code>git worktree</code>s from a single 4.3 MB static Rust binary.<br/>
  Zero-config out of the box for Claude Code, Codex, Gemini CLI, OpenCode, Aider, Goose, and Qwen Code.
</p>

<p align="center">
  <a href="https://github.com/fadiljee/latch-cli/actions/workflows/ci.yml">
    <img src="https://github.com/fadiljee/latch-cli/actions/workflows/ci.yml/badge.svg" alt="CI" />
  </a>
  <img src="https://img.shields.io/badge/status-v0.1.0--dev-orange" alt="status" />
  <img src="https://img.shields.io/badge/rust-stable%20(edition%202024)-blue" alt="rust" />
  <img src="https://img.shields.io/badge/binary-4.3%20MB-brightgreen" alt="binary size" />
  <img src="https://img.shields.io/badge/idle%20RAM-~10.3%20MB-brightgreen" alt="idle ram" />
</p>

---

## Why Latch?

Running multiple AI coding agent CLIs (`claude`, `codex`, `gemini`, `opencode`, `aider`, `goose`, `qwen`, or custom scripts) in parallel usually leads to three recurring problems:

1. **Heavy RAM usage** — Electron/Chromium-based orchestrators consume hundreds of MB to several GB as sessions grow.
2. **Workspace conflicts** — agents editing the same working directory overwrite each other's changes.
3. **Manual worktree management** — creating branches, inspecting diffs, checking merge conflicts, and cleaning up worktrees by hand is tedious.

**Latch** bundles both the **daemon** and **CLI** into a single lightweight Rust executable (`latch-cli` / daemon alias `latchd`) with built-in profiles for all major agents and automatic daemon startup.

---

## Quick Start

### 1. Install or run via `npx`

```bash
# Run directly from GitHub
npx github:fadiljee/latch-cli --help

# Or install globally via npm (once published)
npm install -g latch-runtime
latch-cli --help
```

Check which coding agents are installed on your machine:

```bash
latch-cli agents
```

### 2. Sign in with your account (recommended)

No API key required if you already have a subscription account (Claude Pro/Max, ChatGPT Plus/Pro, Google, OpenCode, etc.):

```bash
latch-cli login claude     # Claude Code account login (OAuth)
latch-cli login codex      # OpenAI Codex account login
latch-cli login gemini     # Google / Gemini CLI login
latch-cli login opencode   # OpenCode account login
```

<details>
<summary>Or use an API key instead (optional)</summary>

If you prefer pay-as-you-go API keys, export the environment variable for your chosen agent — Latch forwards it cleanly via `pass_env` without ever storing it in SQLite or logs:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."   # for claude / opencode / aider
export OPENAI_API_KEY="sk-..."          # for codex / opencode / aider
export GEMINI_API_KEY="AIza..."         # for gemini / opencode / aider
```

</details>

### 3. Run an agent immediately

From inside any Git repository, just run:

```bash
# Auto-starts the daemon, auto-registers the current repo, creates a worktree, and attaches
latch-cli run --profile claude --task "Add /health endpoint" --attach
```

Run multiple agents in parallel on the same repository — each gets its own branch (`latch/<project>/<short-id>`) and isolated worktree:

```bash
latch-cli run --profile claude --task "Implement auth middleware"
latch-cli run --profile codex --task "Write unit tests for auth"
latch-cli run --profile gemini --task "Refactor error handling"
```

### 4. Inspect, diff, and merge

```bash
latch-cli session list
latch-cli session attach <session-id>      # Ctrl-] to detach
latch-cli session screen <session-id>
latch-cli session pause <session-id>
latch-cli session resume <session-id>
latch-cli session diff <session-id>
latch-cli session merge-check <session-id>
latch-cli session merge <session-id> --strategy squash
latch-cli session discard <session-id>
```

Pass `--json` to any command for machine-readable output.

---

## Built-in Agent Profiles

All profiles work out of the box without creating any config file:

| Profile | Command | Auth Support |
| :--- | :--- | :--- |
| `claude` | `claude` | Account login (`latch-cli login claude`) or `ANTHROPIC_API_KEY` |
| `codex` | `codex` | Account login (`latch-cli login codex`) or `OPENAI_API_KEY` |
| `gemini` | `gemini` | Account login (`latch-cli login gemini`) or `GEMINI_API_KEY` / `GOOGLE_API_KEY` |
| `opencode` | `opencode` | Account login (`latch-cli login opencode`) or provider API keys |
| `aider` | `aider` | Provider API keys (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `OPENROUTER_API_KEY`) |
| `goose` | `goose session` | `latch-cli login goose` (`goose configure`) or provider API keys |
| `qwen` | `qwen` | Account login or `OPENAI_API_KEY` / `DASHSCOPE_API_KEY` |
| `shell` | `sh` | Interactive shell for testing or custom scripts |

To add custom profiles or override defaults, generate `~/.config/latch/agents.toml`:

```bash
latch-cli init
latch-cli config check
```

---

## Features & Measured Performance (`v0.1`)

- **Zero-Config UX** — built-in profiles for all major agents, auto-starting daemon, and automatic Git repo registration.
- **Persistent State (SQLite WAL)** — `rusqlite` (`bundled`), single-writer actor thread, automatic migrations, and startup crash reconciliation.
- **Automatic `git worktree` Isolation** — paginated diffs, conflict simulation (`git merge-tree --write-tree`), and idempotent squash/merge or discard.
- **Multi-Session PTY Pool** — `portable-pty` on dedicated OS threads, 12 ms output batching, 1 MiB ring buffer, rolling logs, `vt100` screen snapshots, and process-group control (`SIGSTOP`/`SIGCONT`/`SIGTERM`→`SIGKILL`).
- **Security & Privacy Foundations** — `0600` Unix Domain Socket with `SO_PEERCRED` UID verification, automatic secret/API-key redaction in logs and audit records, and a zero-OpenSSL dependency tree (`rustls` + `rusqlite bundled`).

| Metric | Measured (`v0.1` release) | Budget |
| :--- | ---: | ---: |
| Idle daemon RSS | **~10.3 MB** (10,552 kB) | ≤ 25 MB |
| Overhead per active PTY session | **~316 KB** | ≤ 5 MB |
| Release binary size | **4.3 MB** | < 30 MB |

---

## Platform Support

| Platform | Status (`v0.1`) | Notes |
| :--- | :--- | :--- |
| **Linux (x86_64)** | Supported | Primary target; static `musl` binary verified in CI; Landlock sandbox planned for `v0.3` |
| **macOS** | Partial / Experimental | Shares Unix socket & POSIX process-group foundations |
| **Windows** | WSL2 only | Native Windows support is not yet implemented |

---

## Build from Source (Rust)

Requirements: **Rust** stable (edition 2024) and **Git** ≥ `2.38`.

```bash
git clone https://github.com/fadiljee/latch-cli.git
cd latch-cli

# Standard release build
cargo build --release -p latch-cli

# (Optional) Fully static musl build on Linux
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl -p latch-cli
./scripts/check-static.sh target/x86_64-unknown-linux-musl/release/latch-cli
```

---

## Testing & Verification

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
./scripts/bench-rss.sh
```

---

## Roadmap

- **v0.1 (Done):** UDS daemon, SQLite WAL, `git worktree` + diff/merge, PTY pool, single-binary CLI (`latch-cli`), built-in multi-agent profiles + account login, `npx`/`npm` wrapper, static `musl` CI.
- **v0.2:** Local LLM reverse proxy (real-time token/cost tracking, circuit breaker), embedded Web PWA (`rust-embed`), notifications, remote tunnel.
- **v0.3:** Landlock OS sandbox (`pre_exec`), sensitive path denylist, Approval Gate.
- **v1.0:** `workflow.toml` DAG pipelines, auto-retry gate, multi-model fan-out arena, idle session hibernation.
