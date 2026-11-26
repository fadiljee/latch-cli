<p align="center">
  <img src="assets/icon.png" alt="Latch Logo" width="128" />
</p>

<h1 align="center">Latch</h1>

<p align="center">
  <strong>Autonomous Rust Agent Runtime</strong> — orkestrator <em>AI coding agent</em> berbasis 100% Rust dalam satu <em>static binary</em>.<br/>
  Hemat RAM, terisolasi per <code>git worktree</code>, terukur, dan dikendalikan langsung dari CLI maupun TUI.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/status-v0.1.0--dev-orange" alt="status" />
  <img src="https://img.shields.io/badge/rust-stable%20(edition%202024)-blue" alt="rust" />
  <img src="https://img.shields.io/badge/binary-~4.6%20MB-brightgreen" alt="binary size" />
  <img src="https://img.shields.io/badge/idle%20RAM-~10.5%20MB-brightgreen" alt="idle ram" />
</p>

---

## Kenapa Latch?

Menjalankan banyak CLI agen AI (`claude`, `codex`, `opencode`, `gemini`, atau skrip kustom) secara bersamaan sering memunculkan masalah:

1. **Boros RAM** — orkestrator berbasis Electron/Chromium menyedot ratusan MB hingga GB saat banyak sesi aktif.
2. **Konflik kerja** — agen yang bekerja di direktori yang sama saling menimpa perubahan.
3. **Sulit dipantau** — berpindah-pindah tab terminal untuk memeriksa status, diff, dan merge tidak efisien.

**Latch** menyatukan **daemon**, **CLI**, dan **TUI** dalam **satu executable Rust (`latch` / `latchd`)** tanpa dependensi runtime eksternal (tanpa Node.js, Electron, atau Docker).

---

## Fitur (`v0.1`)

- **Single Binary (`latch` & alias `latchd`)** — mode daemon, klien CLI, dan dashboard TUI dalam satu binary.
- **Persistent State (SQLite WAL)** — `rusqlite` (`bundled`), aktor penulis tunggal, migrasi otomatis, dan rekonsiliasi startup setelah crash.
- **Isolasi `git worktree` Otomatis** — tiap sesi mendapat worktree & branch unik (`latch/<project>/<short-id>`), diff paginasi, simulasi konflik (`git merge-tree --write-tree`), serta squash/merge atau discard idempoten.
- **Multi-Session PTY Pool** — `portable-pty` di OS thread khusus, batching output 12 ms, ring buffer 1 MiB, log rolling, layar virtual `vt100`, serta kontrol process group (`SIGSTOP`/`SIGCONT`/`SIGTERM`→`SIGKILL`).
- **TUI Interaktif (`latch tui`)** — berbasis `ratatui` + `tui-term` dengan daftar sesi, panel PTY live, scrollback, diff popup, dan aksi cepat.
- **Keamanan & Privasi Dasar** — socket UDS `0600` dengan verifikasi `SO_PEERCRED`, redaksi otomatis pola API key/token pada log & audit, serta arsitektur zero-OpenSSL (`rustls` + `rusqlite bundled`).

### Benchmark Terukur (`v0.1` release)

| Metrik | Terukur | Target Anggaran |
| :--- | ---: | ---: |
| Daemon idle RSS | **~10.5 MB** (10,720 kB) | ≤ 25 MB |
| Overhead per sesi PTY aktif | **~328 KB** | ≤ 5 MB |
| Ukuran binary release | **4.6 MB** | < 30 MB |

---

## Struktur Workspace

```text
latch-runtime/
├── Cargo.toml              # workspace root & shared dependencies
├── cli/                    # entrypoint tunggal binary `latch`
├── crates/
│   ├── core/               # model domain, config parser, trait SandboxProvider, protokol UDS
│   ├── store/              # SQLite (rusqlite bundled), migrasi, aktor penyimpanan
│   ├── git-engine/         # worktree lifecycle, diff, merge-tree check, squash/merge
│   ├── pty-pool/           # portable-pty, batching 12 ms, ring buffer, vt100, killpg
│   ├── sandbox/            # trait SandboxProvider (v0.1: NoopProvider; v0.3: Landlock)
│   ├── daemon/             # server UDS JSON-RPC, SessionManager, rekonsiliasi, redaksi log
│   ├── tui/                # antarmuka terminal ratatui + tui-term
│   ├── proxy/              # (Fase 2) local LLM reverse proxy, token/biaya, circuit breaker
│   └── dag/                # (Fase 4) eksekutor pipeline workflow.toml & fan-out arena
└── scripts/
    ├── bench-rss.sh        # benchmark anggaran RAM idle & per sesi
    └── check-static.sh     # verifikasi binary statis musl & batas ukuran
```

---

## Prasyarat

- **Rust** stable (edition 2024)
- **Git** ≥ `2.38` (dibutuhkan untuk `git merge-tree --write-tree`)
- *(Opsional)* `musl-tools` untuk membangun binary statis penuh (`x86_64-unknown-linux-musl`)

---

## Instalasi & Build

```bash
git clone https://github.com/fadiljee/latch-runtime.git
cd latch-runtime

# Build release
cargo build --release -p latch

# (Opsional) Build static binary penuh dengan musl
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl -p latch
./scripts/check-static.sh target/x86_64-unknown-linux-musl/release/latch
```

Binary hasil build tersedia di `target/release/latch`.

---

## Quick Start

### 1. Buat profil agen (`~/.config/latch/agents.toml`)

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

Validasi konfigurasi:

```bash
latch config check
```

### 2. Jalankan daemon dan proyek

```bash
# Mulai daemon di background (atau --foreground untuk systemd)
latch daemon start

# Daftarkan repositori Git
latch project add my-repo /path/to/my-repo

# Jalankan agen pada worktree terisolasi baru
latch run --project my-repo --profile coder-claude --task "Tambahkan endpoint /health"
```

### 3. Pantau lewat TUI atau CLI

```bash
# Buka dashboard TUI interaktif
latch tui

# Atau kelola via CLI
latch session list
latch session attach <session-id>
latch session diff <session-id>
latch session merge-check <session-id>
latch session merge <session-id> --strategy squash
latch session discard <session-id>
```

### Pintasan Keyboard TUI (`latch tui`)

| Tombol | Aksi |
| :--- | :--- |
| `j` / `k` atau `↑` / `↓` | Pilih sesi |
| `Enter` | Masuk mode attach (input diteruskan ke PTY; `Ctrl-]` untuk detach) |
| `PgUp` / `PgDn` | Scrollback output terminal |
| `p` | Pause / Resume sesi (`SIGSTOP` / `SIGCONT`) |
| `d` | Buka popup diff worktree (`j`/`k` scroll, `Esc` tutup) |
| `m` | Squash-merge ke branch utama (dengan konfirmasi `y/n`) |
| `D` | Discard worktree & branch (dengan konfirmasi `y/n`) |
| `x` | Kill sesi (dengan konfirmasi `y/n`) |
| `q` | Keluar dari TUI |

---

## Pengujian & Verifikasi

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./scripts/bench-rss.sh
```

---

## Roadmap

- **v0.1 (Selesai):** Daemon UDS, SQLite WAL, `git worktree` + diff/merge, PTY pool, CLI, TUI.
- **v0.2:** Local LLM reverse proxy (token/biaya real-time, circuit breaker), Embedded Web PWA (`rust-embed`), notifikasi, remote tunnel.
- **v0.3:** OS Sandbox Landlock (`pre_exec`), denylist path sensitif, Approval Gate (TUI & Web).
- **v1.0:** Pipeline DAG (`workflow.toml`), auto-retry gate, fan-out arena multi-model, hibernasi sesi idle.
