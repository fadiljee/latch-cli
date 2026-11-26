<p align="center">
  <img src="assets/icon.png" alt="Latch Logo" width="128" />
</p>

<h1 align="center">Latch</h1>

<p align="center">
  <strong>Autonomous Rust Agent Runtime</strong> — a 100% Rust AI coding agent orchestrator packed into a single static binary.<br/>
  Low memory footprint, isolated <code>git worktree</code> per task, and controllable via CLI or TUI.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/status-v0.1.0--dev-orange" alt="status" />
  <img src="https://img.shields.io/badge/rust-stable%20(edition%202024)-blue" alt="rust" />
  <img src="https://img.shields.io/badge/binary-~4.6%20MB-brightgreen" alt="binary size" />
  <img src="https://img.shields.io/badge/idle%20RAM-~10.5%20MB-brightgreen" alt="idle ram" />
</p>

---

## Why Latch?

Running multiple AI coding agent CLIs (`claude`, `codex`, `opencode`, `gemini`, or custom scripts) in parallel usually leads to three recurring problems:

1. **Heavy RAM usage** — Electron/Chromium-based orchestrators consume hundreds of MB to several GB as sessions grow.
2. **Workspace conflicts** — agents editing the same working directory overwrite each other's changes.
3. **Fragmented control** — juggling terminal tabs to monitor output, inspect diffs, and merge work slows you down.

**Latch** bundles the **daemon**, **CLI**, and **TUI** into a **single Rust executable (`latch` / `latchd`)** with zero external runtime dependencies (no Node.js, Electron, or Docker).

---

## Features (`v0.1`)

- **Single Binary (`latch` & `latchd` alias)** — daemon mode, one-shot CLI client, and interactive TUI in one executable.
- **Persistent State (SQLite WAL)** — `rusqlite` (`bundled`), single-writer actor thread, automatic migrations, and startup crash reconciliation.
- **Automatic `git worktree` Isolation** — each session gets a dedicated worktree and branch (`latch/<project>/<short-id>`), paginated diffs, conflict simulation (`git merge-tree --write-tree`), and idempotent squash/merge or discard.
- **Multi-Session PTY Pool** — `portable-pty` on dedicated OS threads, 12 ms output batching, 1 MiB ring buffer, rolling logs, `vt100` virtual screen snapshots, and process-group control (`SIGSTOP`/`SIGCONT`/`SIGTERM`→`SIGKILL`).
- **Interactive TUI (`latch tui`)** — powered by `ratatui` + `tui-term` with a live session list, embedded PTY viewport, scrollback, diff popup, and confirmation dialogs.
- **Security & Privacy Foundations** — `0600` Unix Domain Socket with `SO_PEERCRED` UID verification, automatic secret/API-key redaction in logs and audit records, and a zero-OpenSSL dependency tree (`rustls` + `rusqlite bundled`).

### Measured Performance (`v0.1` release)

| Metric | Measured | Budget |
| :--- | ---: | ---: |
| Idle daemon RSS | **~10.5 MB** (10,720 kB) | ≤ 25 MB |
| Overhead per active PTY session | **~328 KB** | ≤ 5 MB |
| Release binary size | **4.6 MB** | < 30 MB |

---

## Platform Support

| Platform | Status (`v0.1`) | Notes |
| :--- | :--- | :--- |
| **Linux (x86_64)** | Supported | Primary target; static `musl` build supported; Landlock sandbox planned for `v0.3` |
| **macOS** | Partial / Experimental | Shares Unix socket & POSIX process-group foundations; CI build verification planned |
| **Windows** | WSL2 only | Native Windows support (IPC + ConPTY/Job Object process control) is not yet implemented |

---

## Workspace Structure

```text
latch-runtime/
├── Cargo.toml              # workspace root & shared dependencies
├── cli/                    # single-binary entrypoint (`latch`)
├── crates/
│   ├── core/               # domain models, config parser, SandboxProvider trait, UDS protocol
│   ├── store/              # SQLite (rusqlite bundled), migrations, single-writer actor
│   ├── git-engine/         # worktree lifecycle, diff, merge-tree check, squash/merge
│   ├── pty-pool/           # portable-pty, 12 ms batching, ring buffer, vt100, killpg
│   ├── sandbox/            # SandboxProvider implementations (v0.1: NoopProvider; v0.3: Landlock)
│   ├── daemon/             # UDS JSON-RPC server, SessionManager, reconciliation, log redaction
│   ├── tui/                # ratatui + tui-term terminal UI
│   ├── proxy/              # (Phase 2) local LLM reverse proxy, token/cost tracking, circuit breaker
│   └── dag/                # (Phase 4) workflow.toml DAG executor & fan-out arena
└── scripts/
    ├── bench-rss.sh        # idle & per-session RSS budget benchmark
    └── check-static.sh     # static musl linkage & < 30 MB size check
```

---

## Requirements

- **Rust** stable (edition 2024)
- **Git** ≥ `2.38` (required for `git merge-tree --write-tree`)
- *(Optional)* `musl-tools` for building a fully static Linux binary (`x86_64-unknown-linux-musl`)

---

## Build & Install

```bash
git clone https://github.com/fadiljee/latch-runtime.git
cd latch-runtime

# Standard release build
cargo build --release -p latch

# (Optional) Fully static musl build on Linux
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl -p latch
./scripts/check-static.sh target/x86_64-unknown-linux-musl/release/latch
```

The compiled binary is placed at `target/release/latch`.

---

## Quick Start

### 1. Define agent profiles (`~/.config/latch/agents.toml`)

```toml
[profiles.coder-claude]
command = "claude"
args = []
proxy_mode = "bypass"
pass_env = ["ANTHROPIC_API_KEY"]

[profiles.coder-codex]
command = "codex"
args = []
proxy_mode = "bypass"
pass_env = ["OPENAI_API_KEY"]

[profiles.shell-test]
command = "sh"
args = ["-c", "echo hello > result.txt; sleep 1"]
proxy_mode = "bypass"
```

Validate your configuration:

```bash
latch config check
```

### 2. Start the daemon and register a project

```bash
# Start the daemon in the background (or pass --foreground for systemd)
latch daemon start

# Register a Git repository
latch project add my-repo /path/to/my-repo

# Launch an agent in a fresh isolated worktree
latch run --project my-repo --profile coder-claude --task "Add /health endpoint"
```

### 3. Monitor via TUI or CLI

```bash
# Open the interactive TUI dashboard
latch tui

# Or manage sessions from the CLI
latch session list
latch session attach <session-id>
latch session diff <session-id>
latch session merge-check <session-id>
latch session merge <session-id> --strategy squash
latch session discard <session-id>
```

### TUI Keyboard Shortcuts (`latch tui`)

| Key | Action |
| :--- | :--- |
| `j` / `k` or `↑` / `↓` | Select session |
| `Enter` | Enter attach mode (forwards keystrokes to PTY; `Ctrl-]` to detach) |
| `PgUp` / `PgDn` | Scroll PTY output history |
| `p` | Pause / Resume session (`SIGSTOP` / `SIGCONT`) |
| `d` | Open worktree diff popup (`j`/`k` to scroll, `Esc` to close) |
| `m` | Squash-merge into default branch (`y/n` confirmation) |
| `D` | Discard worktree & branch (`y/n` confirmation) |
| `x` | Kill running session (`y/n` confirmation) |
| `q` | Quit TUI |

---

## Testing & Verification

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./scripts/bench-rss.sh
```

---

## Roadmap

- **v0.1 (Done):** UDS daemon, SQLite WAL, `git worktree` + diff/merge, PTY pool, CLI, TUI.
- **v0.2:** Local LLM reverse proxy (real-time token/cost tracking, circuit breaker), embedded Web PWA (`rust-embed`), notifications, remote tunnel.
- **v0.3:** Landlock OS sandbox (`pre_exec`), sensitive path denylist, Approval Gate (TUI & Web).
- **v1.0:** `workflow.toml` DAG pipelines, auto-retry gate, multi-model fan-out arena, idle session hibernation.
