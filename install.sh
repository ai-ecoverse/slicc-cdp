#!/bin/sh
set -eu

REPO="ai-ecoverse/slicc-cdp"
BINS="playwright-cli curlwright"

print_usage() {
  printf "Usage: sh install.sh [OPTIONS]\n\n"
  printf "Options:\n"
  printf "  --version VERSION   Install a specific version (default: latest)\n"
  printf "  --dir DIR           Installation directory (default: \$HOME/.local/bin)\n"
  printf "  --only BIN          Install only playwright-cli or curlwright\n"
  printf "  --help              Show this help\n"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --dir) INSTALL_DIR="$2"; shift 2 ;;
    --only) BINS="$2"; shift 2 ;;
    --help) print_usage; exit 0 ;;
    *) printf "unknown option: %s\n" "$1" >&2; exit 1 ;;
  esac
done

INSTALL_DIR="${INSTALL_DIR:-${HOME}/.local/bin}"

detect_os() {
  case "$(uname -s)" in
    Darwin) printf "darwin" ;;
    Linux) printf "linux" ;;
    *) printf "Unsupported OS: %s\nInstall with: npm i -g @ai-ecoverse/slicc-cdp\n" "$(uname -s)" >&2; exit 1 ;;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64) printf "amd64" ;;
    arm64|aarch64) printf "arm64" ;;
    *) printf "Unsupported architecture: %s\n" "$(uname -m)" >&2; exit 1 ;;
  esac
}

fetch() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1"
  else
    printf "curl or wget is required\n" >&2
    exit 1
  fi
}

checksum_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f1
  else
    printf "sha256sum or shasum is required\n" >&2
    exit 1
  fi
}

OS=$(detect_os)
ARCH=$(detect_arch)

if [ -z "${VERSION:-}" ]; then
  if command -v curl >/dev/null 2>&1; then
    location=$(curl -sI "https://github.com/${REPO}/releases/latest" 2>/dev/null | grep -i '^location:' | head -1 | tr -d '\r')
  elif command -v wget >/dev/null 2>&1; then
    location=$(wget --spider -S "https://github.com/${REPO}/releases/latest" 2>&1 | grep -i '  location:' | tail -1 | tr -d '\r')
  else
    printf "curl or wget is required\n" >&2
    exit 1
  fi
  VERSION="${location##*/}"
  VERSION="${VERSION#v}"
  if [ -z "$VERSION" ]; then
    printf "could not determine latest version\n" >&2
    exit 1
  fi
fi

BASE_URL="${SLICC_INSTALL_BASE_URL:-https://github.com/${REPO}/releases/download/v${VERSION}}"

tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

fetch "${BASE_URL}/SHA256SUMS" "${tmpdir}/SHA256SUMS"

for bin in ${BINS}; do
  asset="${bin}-${OS}-${ARCH}"
  fetch "${BASE_URL}/${asset}" "${tmpdir}/${asset}"

  expected=$(grep "  ${asset}\$" "${tmpdir}/SHA256SUMS" | cut -d ' ' -f1) || true
  if [ -z "${expected}" ]; then
    printf "no checksum found for %s in SHA256SUMS\n" "${asset}" >&2
    exit 1
  fi

  actual=$(checksum_of "${tmpdir}/${asset}")
  if [ "${actual}" != "${expected}" ]; then
    printf "checksum mismatch for %s\nexpected: %s\nactual:   %s\n" "${asset}" "${expected}" "${actual}" >&2
    exit 1
  fi

  chmod 755 "${tmpdir}/${asset}"
done

mkdir -p "${INSTALL_DIR}"

for bin in ${BINS}; do
  asset="${bin}-${OS}-${ARCH}"
  mv "${tmpdir}/${asset}" "${INSTALL_DIR}/${bin}"
  "${INSTALL_DIR}/${bin}" --version
done

case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *) printf "add %s to your PATH:\n  export PATH=\"%s:\$PATH\"\n" "${INSTALL_DIR}" "${INSTALL_DIR}" ;;
esac
