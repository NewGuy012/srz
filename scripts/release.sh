#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${REPO_ROOT}"
trap 'rm -f Cargo.toml.tmp' EXIT

# Ensure cargo is in PATH
if ! command -v cargo >/dev/null 2>&1 && [ -f "$HOME/.cargo/bin/cargo" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

# Default options
DRY_RUN=false
SKIP_TESTS=false
AUTO_YES=false
FORCE=false
REQUESTED_VER=""

# Parse flags & arguments
while [ $# -gt 0 ]; do
  case "$1" in
    -n|--dry-run)
      DRY_RUN=true
      shift
      ;;
    -y|--yes)
      AUTO_YES=true
      shift
      ;;
    -f|--force)
      FORCE=true
      shift
      ;;
    --skip-tests)
      SKIP_TESTS=true
      shift
      ;;
    -h|--help)
      echo "Usage: ./scripts/release.sh [OPTIONS] [VERSION | patch | minor | major]"
      echo ""
      echo "Automates version bump, verification, commit, tag, and push to trigger CI release."
      echo ""
      echo "Arguments:"
      echo "  VERSION       Explicit version (e.g. 0.6.0, v0.6.0)"
      echo "  patch         Increment patch version (e.g. 0.6.0 -> 0.6.1) [default]"
      echo "  minor         Increment minor version (e.g. 0.6.0 -> 0.7.0)"
      echo "  major         Increment major version (e.g. 0.6.0 -> 1.0.0)"
      echo ""
      echo "Options:"
      echo "  -n, --dry-run     Show what would be done without modifying git or pushing"
      echo "  -f, --force       Overwrite existing local/remote tag if re-triggering a release"
      echo "  -y, --yes         Skip interactive confirmation prompt"
      echo "      --skip-tests  Skip running cargo test before releasing"
      echo "  -h, --help        Show this help message"
      exit 0
      ;;
    *)
      if [ -z "${REQUESTED_VER}" ]; then
        REQUESTED_VER="$1"
      else
        echo "Error: Unexpected argument '$1'" >&2
        exit 1
      fi
      shift
      ;;
  esac
done

# Read current version from Cargo.toml ([package] section)
CURRENT_VER=$(awk '
  /^\[package\]/ { in_pkg=1; next }
  /^\[/ { in_pkg=0 }
  in_pkg && /^version *= *"/ {
    sub(/^version *= *"/, "");
    sub(/".*$/, "");
    print;
    exit;
  }
' Cargo.toml)
if [ -z "${CURRENT_VER}" ]; then
  echo "Error: Could not determine current version from Cargo.toml" >&2
  exit 1
fi

CORE_VER="${CURRENT_VER%%-*}"
CORE_VER="${CORE_VER%%+*}"
MAJOR=$(echo "${CORE_VER}" | cut -d. -f1)
MINOR=$(echo "${CORE_VER}" | cut -d. -f2)
PATCH=$(echo "${CORE_VER}" | cut -d. -f3)

# Calculate target version
case "${REQUESTED_VER}" in
  ""|patch)
    NEW_VER="${MAJOR}.${MINOR}.$((PATCH + 1))"
    ;;
  minor)
    NEW_VER="${MAJOR}.$((MINOR + 1)).0"
    ;;
  major)
    NEW_VER="$((MAJOR + 1)).0.0"
    ;;
  *)
    # Explicit version passed
    CLEAN_VER="${REQUESTED_VER#v}"
    if ! printf '%s\n' "${CLEAN_VER}" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'; then
      echo "Error: Invalid version format '${REQUESTED_VER}'. Must be semver (e.g. '0.6.0')." >&2
      exit 1
    fi
    NEW_VER="${CLEAN_VER}"
    ;;
esac

TAG="v${NEW_VER}"

# Ensure no uncommitted edits (other than docs/src/changelog.md) are left out of the release tag
DIRTY_FILES=$(git status --porcelain --untracked-files=no | grep -v ' docs/src/changelog\.md$' || true)
if [ -n "${DIRTY_FILES}" ]; then
  echo "Error: Working tree has uncommitted changes outside docs/src/changelog.md:" >&2
  echo "${DIRTY_FILES}" >&2
  echo "Please commit or stash your changes before cutting a release." >&2
  exit 1
fi

# Check current git branch
CURRENT_BRANCH=$(git branch --show-current)
if [ -z "${CURRENT_BRANCH}" ]; then
  echo "Error: Detached HEAD state; check out a branch before releasing." >&2
  exit 1
fi
if [ "${CURRENT_BRANCH}" != "master" ] && [ "${CURRENT_BRANCH}" != "main" ]; then
  echo "Warning: You are currently on branch '${CURRENT_BRANCH}', not 'master'."
  if [ "${AUTO_YES}" != "true" ] && [ "${DRY_RUN}" != "true" ]; then
    read -r -p "Continue release from '${CURRENT_BRANCH}'? [y/N]: " CONFIRM_BRANCH
    if [ "${CONFIRM_BRANCH}" != "y" ] && [ "${CONFIRM_BRANCH}" != "Y" ]; then
      echo "Release aborted."
      exit 1
    fi
  fi
fi

resolve_tag_overwrite() {
  local scope="$1"
  local exists="$2"
  [ "${exists}" = "true" ] || return 1
  if [ "${FORCE}" = "true" ]; then
    return 0
  elif [ "${DRY_RUN}" = "true" ]; then
    echo "Notice: ${scope} git tag '${TAG}' already exists (would require -f / confirmation to overwrite)."
    return 1
  elif [ "${AUTO_YES}" != "true" ]; then
    read -r -p "${scope} git tag '${TAG}' already exists. Delete and re-tag '${TAG}'? [y/N]: " CONFIRM_RETAG
    [ "${CONFIRM_RETAG}" = "y" ] || [ "${CONFIRM_RETAG}" = "Y" ] && return 0
    echo "Release aborted." >&2
    exit 1
  else
    echo "Error: ${scope} git tag '${TAG}' already exists. Use -f / --force to overwrite." >&2
    exit 1
  fi
}

LOCAL_TAG_EXISTS=false
git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null 2>&1 && LOCAL_TAG_EXISTS=true
REMOTE_TAG_EXISTS=false
git ls-remote --exit-code --tags origin "refs/tags/${TAG}" >/dev/null 2>&1 && REMOTE_TAG_EXISTS=true

DELETE_LOCAL_TAG=false
DELETE_REMOTE_TAG=false
resolve_tag_overwrite "Local" "${LOCAL_TAG_EXISTS}" && DELETE_LOCAL_TAG=true
resolve_tag_overwrite "Remote" "${REMOTE_TAG_EXISTS}" && DELETE_REMOTE_TAG=true

echo "=================================================="
echo " srz Release Helper"
echo "=================================================="
echo " Current version : ${CURRENT_VER}"
echo " Target version  : ${NEW_VER} (${TAG})"
echo " Branch          : ${CURRENT_BRANCH}"
if [ "${DRY_RUN}" = "true" ]; then
  echo " Mode            : DRY RUN (no git changes or push)"
fi
echo "=================================================="

# Confirmation prompt (before any destructive tag or file mutations)
if [ "${AUTO_YES}" != "true" ] && [ "${DRY_RUN}" != "true" ]; then
  read -r -p "Ready to proceed with release ${TAG}? [y/N]: " CONFIRM
  if [ "${CONFIRM}" != "y" ] && [ "${CONFIRM}" != "Y" ]; then
    echo "Release cancelled."
    exit 0
  fi
fi

# Step 1: Run verification tests
if [ "${SKIP_TESTS}" != "true" ]; then
  echo ""
  echo "==> Running test suite (cargo test --locked --all-targets)..."
  if [ "${DRY_RUN}" = "true" ]; then
    echo "    (Dry-run: skipping tests)"
  else
    cargo test --locked --all-targets
  fi
fi

# Step 2: Update Cargo.toml if version changed
if [ "${CURRENT_VER}" != "${NEW_VER}" ]; then
  echo ""
  echo "==> Bumping version in Cargo.toml: ${CURRENT_VER} -> ${NEW_VER}..."
  if [ "${DRY_RUN}" = "true" ]; then
    echo "    (Dry-run: would update Cargo.toml to ${NEW_VER})"
  else
    awk -v new_ver="${NEW_VER}" '
      /^\[package\]/ { in_pkg=1 }
      in_pkg && /^\[/ && !/^\[package\]/ { in_pkg=0 }
      in_pkg && !done && /^version *= *"/ {
        print "version = \"" new_ver "\""
        done=1
        next
      }
      { print }
    ' Cargo.toml > Cargo.toml.tmp
    mv Cargo.toml.tmp Cargo.toml

    echo "==> Updating Cargo.lock..."
    cargo update -p srz --offline >/dev/null 2>&1 || cargo check --quiet
  fi
fi

# Step 3: Check changelog.md for the new version section (exact heading match)
if [ -f "docs/src/changelog.md" ]; then
  if ! grep -Fxq "## ${TAG}" docs/src/changelog.md; then
    echo "Warning: No section '## ${TAG}' found in docs/src/changelog.md."
    echo "         GitHub Actions release notes will fall back to auto-generated commits."
  fi
fi

# Step 4: Commit changes
if [ "${DRY_RUN}" = "true" ]; then
  echo ""
  echo "==> [Dry Run] Would commit changes:"
  git status --short
  if [ "${DELETE_LOCAL_TAG}" = "true" ]; then
    echo "==> [Dry Run] Would delete existing local tag: ${TAG}"
  fi
  if [ "${DELETE_REMOTE_TAG}" = "true" ]; then
    echo "==> [Dry Run] Would delete existing remote tag on origin: ${TAG}"
  fi
  echo "==> [Dry Run] Would create tag: ${TAG}"
  echo "==> [Dry Run] Would push commit to origin ${CURRENT_BRANCH}"
  echo "==> [Dry Run] Would push tag ${TAG} to origin"
  echo ""
  echo "Dry run complete! No changes were committed or pushed."
  exit 0
fi

if git diff --quiet Cargo.toml Cargo.lock && { [ ! -f "docs/src/changelog.md" ] || git diff --quiet docs/src/changelog.md; }; then
  echo "==> No release file changes to commit."
else
  echo ""
  echo "==> Committing release prep..."
  git add Cargo.toml Cargo.lock
  if [ -f "docs/src/changelog.md" ] && ! git diff --quiet docs/src/changelog.md; then
    git add docs/src/changelog.md
  fi
  git commit -m "chore: bump version to ${TAG}"
fi

# Step 5: Delete old tag (if confirmed) and create new annotated tag
if [ "${DELETE_LOCAL_TAG}" = "true" ]; then
  echo "Notice: Deleting existing local tag '${TAG}'..."
  git tag -d "${TAG}" >/dev/null
fi
if [ "${DELETE_REMOTE_TAG}" = "true" ]; then
  echo "Notice: Deleting existing remote tag '${TAG}' on origin..."
  git push --delete origin "${TAG}"
fi

echo ""
echo "==> Creating annotated tag ${TAG}..."
git tag -a "${TAG}" -m "Release ${TAG}"

# Step 6: Push to remote
echo ""
echo "==> Pushing ${CURRENT_BRANCH} to origin..."
git push origin "${CURRENT_BRANCH}"

echo "==> Pushing tag ${TAG} to origin..."
git push origin "${TAG}"

echo ""
echo "=================================================="
echo " ✔ Release ${TAG} successfully triggered!"
echo " Track GitHub Actions build & release progress at:"
echo " https://github.com/NewGuy012/srz/actions"
echo "=================================================="
