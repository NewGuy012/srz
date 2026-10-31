#!/usr/bin/env bash
set -e

# Navigate to the repository root
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${REPO_ROOT}"

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

# Read current version from Cargo.toml
CURRENT_VER=$(grep -m1 '^version *= *' Cargo.toml | sed -E 's/version *= *"([^"]+)"/\1/')
if [ -z "${CURRENT_VER}" ]; then
  echo "Error: Could not determine current version from Cargo.toml" >&2
  exit 1
fi

MAJOR=$(echo "${CURRENT_VER}" | cut -d. -f1)
MINOR=$(echo "${CURRENT_VER}" | cut -d. -f2)
PATCH=$(echo "${CURRENT_VER}" | cut -d. -f3)

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
    if ! echo "${CLEAN_VER}" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'; then
      echo "Error: Invalid version format '${REQUESTED_VER}'. Must be semver (e.g. '0.6.0')." >&2
      exit 1
    fi
    NEW_VER="${CLEAN_VER}"
    ;;
esac

TAG="v${NEW_VER}"

# Check current git branch
CURRENT_BRANCH=$(git branch --show-current)
if [ "${CURRENT_BRANCH}" != "master" ] && [ "${CURRENT_BRANCH}" != "main" ]; then
  echo "Warning: You are currently on branch '${CURRENT_BRANCH}', not 'master'."
  if [ "${AUTO_YES}" != "true" ]; then
    read -r -p "Continue release from '${CURRENT_BRANCH}'? [y/N]: " CONFIRM_BRANCH
    if [ "${CONFIRM_BRANCH}" != "y" ] && [ "${CONFIRM_BRANCH}" != "Y" ]; then
      echo "Release aborted."
      exit 1
    fi
  fi
fi

# Check if tag already exists locally or remotely
if git rev-parse "${TAG}" >/dev/null 2>&1; then
  if [ "${FORCE}" = "true" ]; then
    echo "Notice: Deleting existing local tag '${TAG}' (--force)..."
    git tag -d "${TAG}" >/dev/null
  elif [ "${AUTO_YES}" != "true" ]; then
    echo "Warning: Local git tag '${TAG}' already exists."
    read -r -p "Delete and re-create local tag '${TAG}'? [y/N]: " CONFIRM_LOCAL_RETAG
    if [ "${CONFIRM_LOCAL_RETAG}" = "y" ] || [ "${CONFIRM_LOCAL_RETAG}" = "Y" ]; then
      git tag -d "${TAG}" >/dev/null
    else
      echo "Release aborted." >&2
      exit 1
    fi
  else
    echo "Error: Local git tag '${TAG}' already exists. Use -f / --force to overwrite." >&2
    exit 1
  fi
fi

if git ls-remote --tags origin "${TAG}" 2>/dev/null | grep -q "${TAG}"; then
  if [ "${FORCE}" = "true" ]; then
    echo "Notice: Deleting existing remote tag '${TAG}' on origin (--force)..."
    git push --delete origin "${TAG}"
  elif [ "${AUTO_YES}" != "true" ]; then
    echo "Warning: Remote tag '${TAG}' already exists on origin (likely from a previous failed release)."
    echo "Note: Crates.io packages are immutable. If '${TAG}' was already published, retagging will not update crates.io (bump patch/minor instead)."
    read -r -p "Delete and re-tag '${TAG}' on origin to retrigger release? [y/N]: " CONFIRM_REMOTE_RETAG
    if [ "${CONFIRM_REMOTE_RETAG}" = "y" ] || [ "${CONFIRM_REMOTE_RETAG}" = "Y" ]; then
      git push --delete origin "${TAG}"
    else
      echo "Release aborted." >&2
      exit 1
    fi
  else
    echo "Error: Remote tag '${TAG}' already exists on origin. Use -f / --force to overwrite." >&2
    exit 1
  fi
fi

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

# Confirmation prompt
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
  echo "==> Running test suite (cargo test --all-targets)..."
  if [ "${DRY_RUN}" = "true" ]; then
    echo "    (Dry-run: skipping tests)"
  else
    cargo test --all-targets
  fi
fi

# Step 2: Update Cargo.toml if version changed
if [ "${CURRENT_VER}" != "${NEW_VER}" ]; then
  echo ""
  echo "==> Bumping version in Cargo.toml: ${CURRENT_VER} -> ${NEW_VER}..."
  if [ "${DRY_RUN}" = "true" ]; then
    echo "    (Dry-run: would update Cargo.toml to ${NEW_VER})"
  else
    sed "s/^version = \"${CURRENT_VER}\"/version = \"${NEW_VER}\"/" Cargo.toml > Cargo.toml.tmp
    mv Cargo.toml.tmp Cargo.toml

    echo "==> Updating Cargo.lock..."
    cargo check --quiet
  fi
fi

# Step 3: Check CHANGELOG.md for the new version section
if [ -f "docs/CHANGELOG.md" ]; then
  if ! grep -q "^## ${TAG}" docs/CHANGELOG.md; then
    echo "Warning: No section '## ${TAG}' found in docs/CHANGELOG.md."
    echo "         GitHub Actions release notes will fall back to auto-generated commits."
  fi
fi

# Step 4: Commit changes
if [ "${DRY_RUN}" = "true" ]; then
  echo ""
  echo "==> [Dry Run] Would commit changes:"
  git status --short
  echo "==> [Dry Run] Would create tag: ${TAG}"
  echo "==> [Dry Run] Would push commit to origin ${CURRENT_BRANCH}"
  echo "==> [Dry Run] Would push tag ${TAG} to origin"
  echo ""
  echo "Dry run complete! No changes were committed or pushed."
  exit 0
fi

if git diff --quiet Cargo.toml Cargo.lock; then
  echo "==> No Cargo.toml / Cargo.lock changes to commit."
else
  echo ""
  echo "==> Committing version bump..."
  git add Cargo.toml Cargo.lock
  if [ -f "docs/CHANGELOG.md" ] && ! git diff --quiet docs/CHANGELOG.md; then
    git add docs/CHANGELOG.md
  fi
  git commit -m "chore: bump version to ${TAG}"
fi

# Step 5: Create Git Tag
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
