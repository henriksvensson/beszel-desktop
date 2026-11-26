#!/bin/sh
# Beszel Desktop installer — downloads SHA-256-verified Linux release binaries
# into ~/.local/bin and runs `beszel-ctl init` when attached to a terminal.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/henriksvensson/beszel-desktop/main/packaging/install.sh | sh
#
# Options:
#   --no-init           Install binaries only; skip interactive `beszel-ctl init`
#   --version <tag>     Install a specific release tag (e.g. v0.1.0); defaults to latest

set -eu

REPO="henriksvensson/beszel-desktop"
INSTALL_DIR="${BESZEL_INSTALL_DIR:-$HOME/.local/bin}"
RUN_INIT=1
VERSION=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --no-init)
            RUN_INIT=0
            shift
            ;;
        --version)
            [ "$#" -ge 2 ] || { echo "Missing value for --version" >&2; exit 1; }
            VERSION="$2"
            shift 2
            ;;
        -h|--help)
            sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "Error: required command '$1' not found." >&2
        exit 1
    }
}

require_cmd curl
require_cmd tar
require_cmd sha256sum

ARCH="$(uname -m)"
case "$ARCH" in
    x86_64|amd64)
        TARGET="x86_64-unknown-linux-gnu"
        ;;
    aarch64|arm64)
        TARGET="aarch64-unknown-linux-gnu"
        ;;
    *)
        echo "Error: unsupported architecture '$ARCH' (prebuilt binaries support x86_64 and aarch64)." >&2
        exit 1
        ;;
esac

if [ -z "$VERSION" ]; then
    VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
        | head -n 1)"
    if [ -z "$VERSION" ]; then
        echo "Error: failed to resolve latest release tag from GitHub." >&2
        exit 1
    fi
fi

PKG="beszel-desktop-${VERSION}-${TARGET}"
TARBALL="${PKG}.tar.gz"
BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

echo "Downloading Beszel Desktop ${VERSION} (${TARGET})..."
curl -fsSL "${BASE_URL}/${TARBALL}" -o "${TMPDIR}/${TARBALL}"
curl -fsSL "${BASE_URL}/SHA256SUMS" -o "${TMPDIR}/SHA256SUMS"

echo "Verifying SHA-256 checksum..."
(
    cd "$TMPDIR"
    grep "  ${TARBALL}\$" SHA256SUMS > SHA256SUMS.target
    sha256sum -c SHA256SUMS.target
)

tar -xzf "${TMPDIR}/${TARBALL}" -C "$TMPDIR"
mkdir -p "$INSTALL_DIR"
install -Dm755 "${TMPDIR}/${PKG}/beszel-relay" "${INSTALL_DIR}/beszel-relay"
install -Dm755 "${TMPDIR}/${PKG}/beszel-ctl" "${INSTALL_DIR}/beszel-ctl"
echo "Installed beszel-relay and beszel-ctl to ${INSTALL_DIR}"

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        echo "Note: ${INSTALL_DIR} is not on your PATH. Add it to your shell profile if you want to run beszel-ctl directly."
        ;;
esac

if [ "$RUN_INIT" -eq 1 ]; then
    if [ -r /dev/tty ]; then
        echo ""
        "${INSTALL_DIR}/beszel-ctl" init </dev/tty
    else
        echo "Non-interactive session detected. Run '${INSTALL_DIR}/beszel-ctl init' to configure your Beszel hub."
    fi
fi
