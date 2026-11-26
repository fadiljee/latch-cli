<p align="center">
  <img src="assets/icon.png" alt="Latch Logo" width="128" />
</p>

<h1 align="center">Latch Runtime CLI</h1>

<p align="center">
  <strong>Autonomous Rust Agent Runtime CLI</strong> — a 100% Rust AI coding agent orchestrator packed into a single static binary, installable via <code>npx</code> / <code>npm</code>.<br/>
  Low memory footprint, isolated <code>git worktree</code> per task, and full CLI lifecycle control.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/status-v0.1.0--dev-orange" alt="status" />
  <img src="https://img.shields.io/badge/rust-stable%20(edition%202024)-blue" alt="rust" />
  <img src="https://img.shields.io/badge/cli-latch--cli-blueviolet" alt="cli" />
  <img src="https://img.shields.io/badge/idle%20RAM-~10.5%20MB-brightgreen" alt="idle ram" />
</p>

---

## Why Latch?

Running multiple AI coding agent CLIs (`claude`, `codex`, `opencode`, `gemini`, or custom scripts) in parallel usually leads to three recurring problems:

1. **Heavy RAM usage** — Electron/Chromium-based orchestrators consume hundreds of MB to several GB as sessions grow.
2. **Workspace conflicts** — agents editing the same working directory overwrite each other's changes.
3. **Manual worktree management** — creating branches, inspecting diffs, checking merge conflicts, and cleaning up worktrees by hand is tedious.

**Latch** bundles both the **daemon** and **CLI** into a single lightweight Rust executable (`latch-cli` / daemon alias `latchd`) wrapped for effortless `npx` / `npm` installation.

---

## Features (`v0.1`)

- **Single Binary CLI (`latch-cli` & `latchd` alias)** — daemon mode and one-shot CLI client in one executable (named `latch-cli` to avoid conflicts with unrelated desktop apps named `latch`).
- **Install & Run via `npx` / `npm`** — zero-dependency Node.js wrapper fetches prebuilt static binaries from GitHub Releases (or falls back to building locally with Cargo).
- **Persistent State (SQLite WAL)** — `rusqlite` (`bundled`), single-writer actor thread, automatic migrations, and startup crash reconciliation.
- **Automatic `git worktree` Isolation** — each session gets a dedicated worktree and branch (`latch/<project>/<short-id>`), paginated diffs, conflict simulation (`git merge-tree --write-tree`), and idempotent squash/merge or discard.
- **Multi-Session PTY Pool** — `portable-pty` on dedicated OS threads, 12 ms output batching, 1 MiB ring buffer, rolling logs, `vt100` screen snapshots, interactive attach (`Ctrl-]`), and process-group control (`SIGSTOP`/`SIGCONT`/`SIGTERM`→`SIGKILL`).
- **Security & Privacy Foundations** — `0600` Unix Domain Socket with `SO_PEERCRED` UID verification, automatic secret/API-key redaction in logs and audit records, and a zero-OpenSSL dependency tree (`rustls` + `rusqlite bundled`).

---

## Platform Support

| Platform | Status (`v0.1`) | Notes |
| :--- | :--- | :--- |
| **Linux (x86_64)** | Supported | Primary target; static `musl` binary; Landlock sandbox planned for `v0.3` |
| **macOS** | Partial / Experimental | Shares Unix socket & POSIX process-group foundations |
| **Windows** | WSL2 only | Native Windows support is not yet implemented |

---

## Installation

### Via `npx` / `npm`

```bash
# Run directly via npx (once published to npm)
npx latch-runtime --help

# Or install globally via npm
npm install -g latch-runtime
latch-cli --help

# Or run directly from GitHub
npx github:fadiljee/latch-runtime --help
```

### Build from Source (Rust)

Requirements: **Rust** stable (edition 2024) and **Git** ≥ `2.38`.

```bash
git clone https://github.com/fadiljee/latch-runtime.git
cd latch-runtime

# Standard release build
cargo build --release -p latch-cli

# (Optional) Fully static musl build on Linux
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl -p latch-cli
./scripts/check-static.sh target/x86_64-unknown-linux-musl/release/latch-cli
```

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
latch-cli config check
```

### 2. Start the daemon and launch an agent

```bash
# Start daemon in background (or pass --foreground for systemd)
latch-cli daemon start

# Register a Git repository
latch-cli project add my-repo /path/to/my-repo

# Launch an agent in a fresh isolated worktree
latch-cli run --project my-repo --profile coder-claude --task "Add /health endpoint"
```

### 3. Inspect, attach, diff, and merge

```bash
latch-cli session list
latch-cli session screen <session-id>
latch-cli session attach <session-id>      # Ctrl-] to detach
latch-cli session pause <session-id>
latch-cli session resume <session-id>
latch-cli session diff <session-id>
latch-cli session merge-check <session-id>
latch-cli session merge <session-id> --strategy squash
latch-cli session discard <session-id>
latch-cli daemon stop
```

Pass `--json` to any command for machine-readable output.

---

## Workspace Structure

```text
latch-runtime/
├── Cargo.toml              # Rust workspace root & shared dependencies
├── package.json            # npm wrapper package (`latch-runtime`)
├── bin/latch-cli.js        # npx / npm binary shim
├── cli/                    # single-binary CLI entrypoint (`latch-cli`)
├── crates/
│   ├── core/               # domain models, config parser, SandboxProvider trait, UDS protocol
│   ├── store/              # SQLite (rusqlite bundled), migrations, single-writer actor
│   ├── git-engine/         # worktree lifecycle, diff, merge-tree check, squash/merge
│   ├── pty-pool/           # portable-pty, 12 ms batching, ring buffer, vt100, killpg
│   ├── sandbox/            # SandboxProvider implementations (v0.1: NoopProvider; v0.3: Landlock)
│   ├── daemon/             # UDS JSON-RPC server, SessionManager, reconciliation, log redaction
│   ├── proxy/              # (Phase 2) local LLM reverse proxy, token/cost tracking, circuit breaker
│   └── dag/                # (Phase 4) workflow.toml DAG executor & fan-out arena
└── scripts/
    ├── npm-install.js      # postinstall binary downloader / Cargo fallback
    ├── bench-rss.sh        # idle & per-session RSS budget benchmark
    └── check-static.sh     # static musl linkage & < 30 MB size check
```

---

## Testing & Verification

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./scripts/bench-rss.sh
```
