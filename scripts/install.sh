#!/bin/sh
set -e

main() {
  REPO="NewGuy012/srz"
  INSTALL_DIR="${INSTALL_DIR:-${HOME}/.local/bin}"

  # Helper for robust downloads with curl or wget
  download_file() {
    url="$1"
    dest="$2"
    if command -v curl >/dev/null 2>&1; then
      curl -fsSL "${url}" -o "${dest}"
    elif command -v wget >/dev/null 2>&1; then
      wget -q -O "${dest}" "${url}"
    else
      echo "Error: Neither curl nor wget found in PATH. Please install either tool to proceed." >&2
      exit 1
    fi
  }

  # 1. Detect operating system
  OS="$(uname -s)"
  case "${OS}" in
    Darwin) OS_TARGET="apple-darwin" ;;
    Linux)  OS_TARGET="unknown-linux-gnu" ;;
    *)
      echo "Error: Unsupported operating system: ${OS}" >&2
      echo "Please build from source using 'cargo install srz'" >&2
      exit 1
      ;;
  esac

  # 2. Detect machine architecture
  ARCH="$(uname -m)"
  case "${ARCH}" in
    x86_64|amd64) ARCH_TARGET="x86_64" ;;
    arm64|aarch64) ARCH_TARGET="aarch64" ;;
    *)
      echo "Error: Unsupported architecture: ${ARCH}" >&2
      echo "Please build from source using 'cargo install srz'" >&2
      exit 1
      ;;
  esac

  # Intel Mac check with Rosetta 2 translation detection
  if [ "${OS}" = "Darwin" ] && [ "${ARCH_TARGET}" = "x86_64" ]; then
    if [ "$(sysctl -in sysctl.proc_translated 2>/dev/null)" = "1" ] || [ "$(sysctl -in hw.optional.arm64 2>/dev/null)" = "1" ]; then
      echo "Notice: Detected Apple Silicon running under Rosetta emulation. Selecting native aarch64 binary..."
      ARCH_TARGET="aarch64"
    else
      echo "Error: Intel Macs (x86_64) are not supported with prebuilt binaries." >&2
      echo "Prebuilt binaries are only provided for Apple Silicon (arm64/aarch64)." >&2
      echo "Please build and install srz from source using:" >&2
      echo "  cargo install srz" >&2
      exit 1
    fi
  fi

  # Musl libc detection on Linux
  if [ "${OS}" = "Linux" ] && command -v ldd >/dev/null 2>&1; then
    if ldd --version 2>&1 | grep -qi musl; then
      echo "Warning: Detected musl libc (e.g. Alpine Linux). Prebuilt binaries require GNU libc (glibc)."
      echo "If installation fails to run, please build from source using 'cargo install srz'."
    fi
  fi

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
  if [ -z "${TMP_DIR}" ] || [ ! -d "${TMP_DIR}" ]; then
    echo "Error: Failed to create temporary directory." >&2
    exit 1
  fi
  trap 'rm -rf "${TMP_DIR}"' EXIT INT TERM HUP

  if ! download_file "${DOWNLOAD_URL}" "${TMP_DIR}/srz.tar.gz"; then
    echo "Error: Failed to download ${DOWNLOAD_URL}" >&2
    echo "Ensure the release asset exists, or build from source with 'cargo install srz'" >&2
    exit 1
  fi

  # Download and verify SHA256 checksum
  SHA_URL="${DOWNLOAD_URL}.sha256"
  if ! download_file "${SHA_URL}" "${TMP_DIR}/srz.tar.gz.sha256"; then
    if [ -n "${FORCE}" ]; then
      echo "Warning: Could not download SHA256 checksum from ${SHA_URL}; bypassing due to FORCE=1."
    else
      echo "Error: Failed to download SHA256 checksum file from ${SHA_URL}." >&2
      echo "Aborting installation for security. Run with 'FORCE=1 ./scripts/install.sh' to bypass." >&2
      exit 1
    fi
  else
    echo "Verifying SHA256 checksum..."
    EXPECTED_SHA="$(awk '{print $1}' "${TMP_DIR}/srz.tar.gz.sha256" | tr -d '\r' | head -n 1 | tr '[:upper:]' '[:lower:]')"
    if [ -z "${EXPECTED_SHA}" ]; then
      if [ -n "${FORCE}" ]; then
        echo "Warning: SHA256 checksum file was empty; bypassing due to FORCE=1."
      else
        echo "Error: SHA256 checksum file was empty. Aborting installation for security." >&2
        exit 1
      fi
    elif ! echo "${EXPECTED_SHA}" | grep -Eq '^[0-9a-f]{64}$'; then
      if [ -n "${FORCE}" ]; then
        echo "Warning: Invalid SHA256 checksum format; bypassing due to FORCE=1."
      else
        echo "Error: Checksum file does not contain a valid 64-character SHA256 hash." >&2
        echo "Aborting installation for security. Run with 'FORCE=1 ./scripts/install.sh' to bypass." >&2
        exit 1
      fi
    else
      if command -v sha256sum >/dev/null 2>&1; then
        COMPUTED_SHA="$(sha256sum "${TMP_DIR}/srz.tar.gz" | awk '{print $1}' | tr '[:upper:]' '[:lower:]')"
      elif command -v shasum >/dev/null 2>&1; then
        COMPUTED_SHA="$(shasum -a 256 "${TMP_DIR}/srz.tar.gz" | awk '{print $1}' | tr '[:upper:]' '[:lower:]')"
      else
        echo "Warning: Neither sha256sum nor shasum found; cannot compute local hash."
        if [ -z "${FORCE}" ]; then
          echo "Error: Aborting because SHA256 tools are missing. Run with 'FORCE=1' to bypass." >&2
          exit 1
        fi
        COMPUTED_SHA="${EXPECTED_SHA}"
      fi

      if [ "${EXPECTED_SHA}" != "${COMPUTED_SHA}" ]; then
        echo "Error: SHA256 checksum verification failed!" >&2
        echo "Expected: ${EXPECTED_SHA}" >&2
        echo "Computed: ${COMPUTED_SHA}" >&2
        exit 1
      fi
      echo "✔ Checksum verified successfully."
    fi
  fi

  tar -xzf "${TMP_DIR}/srz.tar.gz" -C "${TMP_DIR}"

  if [ ! -f "${TMP_DIR}/srz" ]; then
    echo "Error: Binary 'srz' not found in downloaded archive." >&2
    exit 1
  fi
  chmod +x "${TMP_DIR}/srz"

  mkdir -p "${INSTALL_DIR}"
  # Atomically install binary using an unguessable temp file on the target directory/filesystem
  TMP_TARGET=$(mktemp "${INSTALL_DIR}/.srz.tmp.XXXXXX" 2>/dev/null || echo "${INSTALL_DIR}/.srz.tmp.$$.$RANDOM")
  rm -f "${TMP_TARGET}"
  cp "${TMP_DIR}/srz" "${TMP_TARGET}"
  chmod 0755 "${TMP_TARGET}"
  mv -f "${TMP_TARGET}" "${INSTALL_DIR}/srz"

  echo "✔ Successfully installed srz to ${INSTALL_DIR}/srz"

  # Check if INSTALL_DIR is in PATH
  CLEAN_INSTALL_DIR="${INSTALL_DIR%/}"
  case ":${PATH}:" in
    *:"${CLEAN_INSTALL_DIR}":*|*:"${CLEAN_INSTALL_DIR}/":*) ;;
    *)
      echo ""
      echo "Notice: ${CLEAN_INSTALL_DIR} is not in your current PATH."
      echo "To run srz from anywhere, add it to your shell config (~/.zshrc or ~/.bashrc):"
      echo "  export PATH=\"${CLEAN_INSTALL_DIR}:\$PATH\""
      ;;
  esac
}

main "$@"
