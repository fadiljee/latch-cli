#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

cargo build --release -p latch-cli
BIN="$ROOT_DIR/target/release/latch-cli"

TMP_ROOT="$(mktemp -d)"
DAEMON_PID=""

cleanup() {
  set +e
  if [[ -n "$DAEMON_PID" ]] && kill -0 "$DAEMON_PID" 2>/dev/null; then
    "$BIN" daemon stop >/dev/null 2>&1 || kill "$DAEMON_PID" 2>/dev/null || true
    wait "$DAEMON_PID" 2>/dev/null || true
  fi
  rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

export LATCH_CONFIG_DIR="$TMP_ROOT/cfg"
export LATCH_STATE_DIR="$TMP_ROOT/state"
mkdir -p "$LATCH_CONFIG_DIR" "$LATCH_STATE_DIR" "$TMP_ROOT/repo"

cat >"$LATCH_CONFIG_DIR/agents.toml" <<'EOF'
[profiles.sleeper]
command = "sh"
args = ["-c", "sleep 3600"]
proxy_mode = "bypass"
EOF

git -C "$TMP_ROOT/repo" init -b main >/dev/null
git -C "$TMP_ROOT/repo" config user.name "Bench"
git -C "$TMP_ROOT/repo" config user.email "bench@localhost"
echo "bench" >"$TMP_ROOT/repo/README.md"
git -C "$TMP_ROOT/repo" add README.md
git -C "$TMP_ROOT/repo" commit -m "init" >/dev/null

"$BIN" daemon start --foreground >/dev/null 2>&1 &
DAEMON_PID=$!

SOCK="$LATCH_STATE_DIR/daemon.sock"
for _ in $(seq 1 60); do
  [[ -S "$SOCK" ]] && break
  sleep 0.05
done
[[ -S "$SOCK" ]] || { echo "ERROR: daemon socket did not appear" >&2; exit 1; }

sleep 2
IDLE_RSS_KB="$(awk '/VmRSS:/ {print $2}' "/proc/${DAEMON_PID}/status")"
echo "Idle VmRSS: ${IDLE_RSS_KB} kB (limit: 25600 kB)"
if (( IDLE_RSS_KB > 25600 )); then
  echo "FAIL: idle RSS ${IDLE_RSS_KB} kB > 25600 kB" >&2
  exit 1
fi

"$BIN" project add bench "$TMP_ROOT/repo" >/dev/null
for i in 1 2 3 4 5; do
  "$BIN" run --project bench --profile sleeper --task "bench-$i" >/dev/null
done

sleep 2
RSS5_KB="$(awk '/VmRSS:/ {print $2}' "/proc/${DAEMON_PID}/status")"
DELTA_KB=$(( (RSS5_KB - IDLE_RSS_KB) / 5 ))
echo "5-session VmRSS: ${RSS5_KB} kB | Per-session delta: ${DELTA_KB} kB (limit: 5120 kB)"
if (( DELTA_KB > 5120 )); then
  echo "FAIL: per-session RSS ${DELTA_KB} kB > 5120 kB" >&2
  exit 1
fi

echo "PASS: RSS within budget (idle=${IDLE_RSS_KB} kB, per_session=${DELTA_KB} kB)"
