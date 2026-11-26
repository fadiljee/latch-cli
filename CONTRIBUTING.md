# Contributing to Latch CLI

Thank you for your interest in contributing to **Latch CLI** (`latch-cli`)!

## Prerequisites

- **Rust** stable (edition 2024, MSRV `1.90`), installed via [`rustup`](https://rustup.rs/)
- Optional target for static Linux builds: `rustup target add x86_64-unknown-linux-musl` + `musl-tools`
- **Git** ≥ `2.38` (required for `git merge-tree --write-tree`)
- **Node.js** ≥ `18` (for testing the `npx`/`npm` wrapper)
- Optional developer tools: `cargo-nextest`, `cargo-deny`, `cargo-bloat`

## Local Setup

```bash
git clone https://github.com/fadiljee/latch-cli.git
cd latch-cli

cargo build --workspace
cargo nextest run --workspace   # fallback: cargo test --workspace
```

## Development Workflow

1. Open an issue or comment on an existing one to discuss non-trivial changes before implementing.
2. Create a topic branch from `main`, e.g. `feat/<crate>-<short-name>` or `fix/<crate>-<short-name>`.
3. Write unit and/or integration tests alongside your changes.
4. Run the full verification gate before opening a Pull Request:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
node --check bin/latch-cli.js && node --check scripts/npm-install.js && npm pack --dry-run
```

5. Update `README.md` and `CHANGELOG.md` (under `Unreleased`) when adding user-facing features or behavior changes.

## Coding Standards & Guardrails

- **Zero-OpenSSL / Static `musl`:** Always use `rustls` (never `openssl` or `native-tls`) and keep `rusqlite` on the `bundled` feature.
- **No `unsafe` by default:** `#![forbid(unsafe_code)]` is enforced across the workspace.
- **No `.unwrap()` / `.expect()` in production code:** Propagate or handle errors explicitly (`thiserror` in library crates, `anyhow` at the `cli` / `daemon` boundary). Enforced by workspace Clippy lints.
- **Tokio & PTY safety:**
  - Never hold a `std::sync::Mutex` or `RwLock` guard across an `.await` point.
  - Keep blocking `rusqlite` calls off Tokio worker threads (use the `latch-store` single-writer actor thread).
  - Keep blocking `portable-pty` reads on dedicated OS threads with 8–16 ms output batching and process-group teardown via `killpg`.
- **No credential leaks:** Never log `Authorization` headers, API keys, or full prompts. Route logs and audit messages through `latch_core::redact::redact`.
- **Performance budgets:**
  - Idle daemon RSS ≤ 25 MB
  - Per active PTY session overhead ≤ 5 MB
  - Static `musl` release binary < 30 MB

## Commit Messages

Follow [Conventional Commits](https://www.conventionalcommits.org/): `feat`, `fix`, `docs`, `refactor`, `test`, `perf`, `build`, `ci`, `chore`.

Examples:
- `feat(cli): add /new slash command for parallel sessions`
- `fix(git-engine): handle renamed files in paginated diff`

## Pull Request Template

```markdown
## Summary
<what changed and why>

## How to Test
<commands or reproduction steps>

## Performance & Size Impact
Idle RSS: ... MB | Binary size: ... MB | Notes: ...

## Checklist
- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` / `cargo nextest run --workspace` passes
- [ ] `README.md` / `CHANGELOG.md` updated if applicable
- [ ] No secrets, API keys, or sensitive logs committed
```

## Security Disclosures

Please do **not** open public GitHub issues for security vulnerabilities. Use GitHub's private vulnerability reporting on the repository so the issue can be investigated and patched before disclosure.

## Code of Conduct

Be respectful, constructive, and welcoming. Critique code, not people.
