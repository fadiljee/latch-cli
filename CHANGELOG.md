# Changelog

Format mengikuti [Keep a Changelog](https://keepachangelog.com/id-ID/1.1.0/) dan versi mengikuti [Semantic Versioning](https://semver.org/lang/id/). Selama `0.x`, perubahan kontrak boleh terjadi antar minor.

## [Unreleased]

### Added
- Dokumentasi awal: README, AGENTS, CONTRIBUTING, PRD, REQUIREMENTS, ARCHITECTURE, TECH_STACK, ROADMAP, DATA_MODEL, API, WORKFLOW_SPEC, SECURITY, RISKS, NFR_BUDGET, GLOSSARY, TASKS, USER_GUIDE, dan ADR 0001-0007.
- Fondasi v0.1: workspace Cargo `latch` (9 crate + `cli`), model domain & transisi status (`latch-core`), persistensi SQLite WAL + migrasi 0001 (`latch-store`), mesin worktree/diff/merge-tree (`latch-git`), pool PTY dengan batching 12 ms, ring buffer, dan snapshot `vt100` (`latch-pty`), `NoopProvider` (`latch-sandbox`), daemon UDS JSON-RPC + rekonsiliasi startup + redaksi log (`latch-daemon`), binary tunggal CLI (`latch` / `latchd`), TUI `ratatui` + `tui-term` (`latch-tui`), serta CI & benchmark RSS (`scripts/bench-rss.sh`).

## Rencana rilis

- **0.1.0** Fondasi: daemon, SQLite, worktree, PTY pool, diff/merge, TUI.
- **0.2.0** Observabilitas & Web: proxy LLM, telemetri, circuit breaker, Web PWA, notifikasi, mode remote.
- **0.3.0** Keamanan: Landlock, denylist, trait `SandboxProvider`, approval gate.
- **1.0.0** Otomasi penuh: `workflow.toml`, estafet, retry gate, fan-out arena, hibernasi.

<!--
Templat entri rilis:

## [0.1.0] - YYYY-MM-DD
### Added
### Changed
### Deprecated
### Removed
### Fixed
### Security
-->
