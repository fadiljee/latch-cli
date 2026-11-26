#!/usr/bin/env bash
set -euo pipefail

BIN="${1:?usage: check-static.sh <path-to-binary>}"

if [[ ! -f "$BIN" ]]; then
  echo "ERROR: binary not found: $BIN" >&2
  exit 1
fi

FILE_OUT="$(file "$BIN")"
echo "file: $FILE_OUT"
if [[ "$FILE_OUT" != *"statically linked"* && "$FILE_OUT" != *"static-pie linked"* ]]; then
  echo "ERROR: binary is not statically linked" >&2
  exit 1
fi

LDD_OUT="$(ldd "$BIN" 2>&1 || true)"
echo "ldd: $LDD_OUT"
if [[ "$LDD_OUT" != *"not a dynamic executable"* && "$LDD_OUT" != *"statically linked"* ]]; then
  echo "ERROR: unexpected ldd output" >&2
  exit 1
fi

SIZE_BYTES="$(stat -c%s "$BIN")"
MAX_BYTES=$((30 * 1024 * 1024))
echo "size: ${SIZE_BYTES} bytes (limit ${MAX_BYTES})"
if (( SIZE_BYTES >= MAX_BYTES )); then
  echo "ERROR: binary size ${SIZE_BYTES} >= 30 MB (${MAX_BYTES})" >&2
  exit 1
fi

echo "OK: static binary verified (< 30 MB)"
