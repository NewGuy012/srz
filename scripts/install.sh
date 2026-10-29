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
  if ! echo "${VERSION}" | grep -Eq '^v?[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'; then
    echo "Error: Invalid version format '${VERSION}'. Must match semver (e.g. '0.4.0' or 'v0.4.0')." >&2
    exit 1
  fi
  case "${VERSION}" in
    v*) TAG="${VERSION}" ;;
    *)  TAG="v${VERSION}" ;;
  esac
  ARCHIVE_NAME="srz-${TAG}-${TARGET}.tar.gz"
  DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/${ARCHIVE_NAME}"
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

# Download and verify SHA256 checksum
SHA_URL="${DOWNLOAD_URL}.sha256"
if ! curl -fsSL "${SHA_URL}" -o "${TMP_DIR}/srz.tar.gz.sha256"; then
  if [ -n "${FORCE}" ]; then
    echo "Warning: Could not download SHA256 checksum from ${SHA_URL}; bypassing due to FORCE=1."
  else
    echo "Error: Failed to download SHA256 checksum file from ${SHA_URL}."
    echo "Aborting installation for security. Run with 'FORCE=1 ./scripts/install.sh' to bypass."
    exit 1
  fi
else
  echo "Verifying SHA256 checksum..."
  EXPECTED_SHA="$(awk '{print $1}' "${TMP_DIR}/srz.tar.gz.sha256" | head -n 1)"
  if [ -z "${EXPECTED_SHA}" ]; then
    if [ -n "${FORCE}" ]; then
      echo "Warning: SHA256 checksum file was empty; bypassing due to FORCE=1."
    else
      echo "Error: SHA256 checksum file was empty. Aborting installation for security."
      exit 1
    fi
  else
    if command -v sha256sum >/dev/null 2>&1; then
      COMPUTED_SHA="$(sha256sum "${TMP_DIR}/srz.tar.gz" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
      COMPUTED_SHA="$(shasum -a 256 "${TMP_DIR}/srz.tar.gz" | awk '{print $1}')"
    else
      echo "Warning: Neither sha256sum nor shasum found; cannot compute local hash."
      if [ -z "${FORCE}" ]; then
        echo "Error: Aborting because SHA256 tools are missing. Run with 'FORCE=1' to bypass."
        exit 1
      fi
      COMPUTED_SHA="${EXPECTED_SHA}"
    fi

    if [ "${EXPECTED_SHA}" != "${COMPUTED_SHA}" ]; then
      echo "Error: SHA256 checksum verification failed!"
      echo "Expected: ${EXPECTED_SHA}"
      echo "Computed: ${COMPUTED_SHA}"
      exit 1
    fi
    echo "✔ Checksum verified successfully."
  fi
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
