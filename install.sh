#!/usr/bin/env sh
set -eu

REPO="fadiljee/latch-cli"
INSTALL_DIR="${LATCH_INSTALL_DIR:-$HOME/.local/bin}"

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64|amd64) TARGET="x86_64-unknown-linux-musl" ;;
      aarch64|arm64) TARGET="aarch64-unknown-linux-musl" ;;
      *)
        echo "Unsupported Linux architecture: $ARCH" >&2
        exit 1
        ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      x86_64) TARGET="x86_64-apple-darwin" ;;
      arm64) TARGET="aarch64-apple-darwin" ;;
      *)
        echo "Unsupported macOS architecture: $ARCH" >&2
        exit 1
        ;;
    esac
    ;;
  *)
    echo "Unsupported OS: $OS (on Windows, please use WSL2)." >&2
    exit 1
    ;;
esac

if [ -n "${LATCH_VERSION:-}" ]; then
  TAG="$LATCH_VERSION"
else
  TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
    | head -n 1)"
fi

if [ -z "$TAG" ]; then
  echo "No published GitHub Release found yet for ${REPO}." >&2
  echo "Build from source instead: cargo install --git https://github.com/${REPO}.git latch-cli" >&2
  exit 1
fi

ASSET="latch-cli-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "Downloading latch-cli ${TAG} (${TARGET})..."
curl -fsSL "$URL" -o "${TMP_DIR}/${ASSET}"

if curl -fsSL "https://github.com/${REPO}/releases/download/${TAG}/SHA256SUMS.txt" -o "${TMP_DIR}/SHA256SUMS.txt" 2>/dev/null; then
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$TMP_DIR" && grep " ${ASSET}\$" SHA256SUMS.txt | sha256sum -c - >/dev/null)
  elif command -v shasum >/dev/null 2>&1; then
    (cd "$TMP_DIR" && grep " ${ASSET}\$" SHA256SUMS.txt | shasum -a 256 -c - >/dev/null)
  fi
fi

tar -xzf "${TMP_DIR}/${ASSET}" -C "$TMP_DIR"
mkdir -p "$INSTALL_DIR"
mv -f "${TMP_DIR}/latch-cli" "${INSTALL_DIR}/latch-cli"
chmod 755 "${INSTALL_DIR}/latch-cli"

echo "Installed latch-cli to ${INSTALL_DIR}/latch-cli"
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    echo "Add ${INSTALL_DIR} to your PATH:"
    echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    ;;
esac
echo "Run \`latch-cli\` inside any Git repository to get started."
