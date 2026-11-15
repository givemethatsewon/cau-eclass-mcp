#!/usr/bin/env bash
# Install a prebuilt eclass-mcp Rust binary. No Rust, Python, uv or venv needed.
set -eu

REPO="${ECLASS_REPO:-givemethatsewon/cau-eclass-mcp}"
VERSION="${ECLASS_VERSION:-latest}"
INSTALL_DIR="${ECLASS_INSTALL_DIR:-$HOME/.local/bin}"

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin)
    case "$arch" in
      arm64) target="aarch64-apple-darwin" ;;
      x86_64) target="x86_64-apple-darwin" ;;
      *) echo "Unsupported macOS architecture: $arch" >&2; exit 1 ;;
    esac
    ;;
  Linux)
    case "$arch" in
      x86_64|amd64) target="x86_64-unknown-linux-gnu" ;;
      aarch64|arm64) target="aarch64-unknown-linux-gnu" ;;
      *) echo "Unsupported Linux architecture: $arch" >&2; exit 1 ;;
    esac
    ;;
  MINGW*|MSYS*|CYGWIN*)
    echo "On Windows, use install.ps1 instead." >&2
    exit 1
    ;;
  *)
    echo "Unsupported operating system: $os" >&2
    exit 1
    ;;
esac

asset="eclass-mcp-${target}.tar.gz"
if [ "$VERSION" = "latest" ]; then
  base="https://github.com/${REPO}/releases/latest/download"
else
  base="https://github.com/${REPO}/releases/download/${VERSION}"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Downloading ${asset}..."
curl -fsSL "${base}/${asset}" -o "${tmp}/${asset}"
curl -fsSL "${base}/${asset}.sha256" -o "${tmp}/${asset}.sha256"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$tmp" && sha256sum -c "${asset}.sha256")
else
  (cd "$tmp" && shasum -a 256 -c "${asset}.sha256")
fi

tar -xzf "${tmp}/${asset}" -C "$tmp"
mkdir -p "$INSTALL_DIR"
install -m 755 "${tmp}/eclass-mcp-${target}/eclass-mcp" "${INSTALL_DIR}/eclass-mcp"

echo "Installed to ${INSTALL_DIR}/eclass-mcp"
"${INSTALL_DIR}/eclass-mcp" --version
case ":$PATH:" in
  *":${INSTALL_DIR}:"*) ;;
  *) echo "Add this to your shell profile: export PATH=\"${INSTALL_DIR}:\$PATH\"" >&2 ;;
esac
printf 'Next: "%s/eclass-mcp" onboard --client claude\n' "$INSTALL_DIR"
