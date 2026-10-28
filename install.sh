#!/bin/sh
set -e

REPO="NewGuy012/srz"
INSTALL_DIR="${INSTALL_DIR:-${HOME}/.local/bin}"

# 1. Detect operating system
OS="$(uname -s)"
case "${OS}" in
  Darwin) OS_TARGET="apple-darwin" ;;
  Linux)  OS_TARGET="unknown-linux-gnu" ;;
  *)
    echo "Error: Unsupported operating system: ${OS}"
    echo "Please build from source using 'cargo install --path .'"
    exit 1
    ;;
esac

# 2. Detect machine architecture
ARCH="$(uname -m)"
case "${ARCH}" in
  x86_64|amd64) ARCH_TARGET="x86_64" ;;
  arm64|aarch64) ARCH_TARGET="aarch64" ;;
  *)
    echo "Error: Unsupported architecture: ${ARCH}"
    echo "Please build from source using 'cargo install --path .'"
    exit 1
    ;;
esac

TARGET="${ARCH_TARGET}-${OS_TARGET}"

# Determine release download URL
if [ -n "${VERSION}" ]; then
  ARCHIVE_NAME="srz-${VERSION}-${TARGET}.tar.gz"
  DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE_NAME}"
else
  # Use latest release asset
  ARCHIVE_NAME="srz-${TARGET}.tar.gz"
  DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/${ARCHIVE_NAME}"
fi

echo "Installing srz for ${TARGET}..."
TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t 'srz-install')"
trap 'rm -rf "${TMP_DIR}"' EXIT

if ! curl -fsSL "${DOWNLOAD_URL}" -o "${TMP_DIR}/srz.tar.gz"; then
  echo "Error: Failed to download ${DOWNLOAD_URL}"
  echo "Ensure the release asset exists, or build from source with 'cargo install --path .'"
  exit 1
fi

tar -xzf "${TMP_DIR}/srz.tar.gz" -C "${TMP_DIR}"

mkdir -p "${INSTALL_DIR}"
mv "${TMP_DIR}/srz" "${INSTALL_DIR}/srz"
chmod +x "${INSTALL_DIR}/srz"

echo "✔ Successfully installed srz to ${INSTALL_DIR}/srz"

# Check if INSTALL_DIR is in PATH
case ":${PATH}:" in
  *:"${INSTALL_DIR}":*) ;;
  *)
    echo ""
    echo "Notice: ${INSTALL_DIR} is not in your current PATH."
    echo "To run srz from anywhere, add it to your shell config (~/.zshrc or ~/.bashrc):"
    echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    ;;
esac
