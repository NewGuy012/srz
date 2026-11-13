# Installation

`srz` provides multiple installation methods across macOS, Linux, and Windows.

---

## 1. Quick Install (One-Line Script)

### macOS & Linux

```bash
# Install latest release to ~/.local/bin/srz
curl -LsSf https://raw.githubusercontent.com/NewGuy012/srz/master/scripts/install.sh | sh

# Install to a custom directory (e.g. /usr/local/bin)
curl -LsSf https://raw.githubusercontent.com/NewGuy012/srz/master/scripts/install.sh | INSTALL_DIR=/usr/local/bin sh

# Install a specific pinned release version
curl -LsSf https://raw.githubusercontent.com/NewGuy012/srz/master/scripts/install.sh | VERSION=0.7.9 sh
```

### Windows (PowerShell)

Run the following in PowerShell:

```powershell
# Install latest release to $env:USERPROFILE\.local\bin\srz.exe
powershell -ExecutionPolicy ByPass -c "irm https://raw.githubusercontent.com/NewGuy012/srz/master/scripts/install.ps1 | iex"

# Install a specific pinned release version
$env:VERSION="0.7.9"; irm https://raw.githubusercontent.com/NewGuy012/srz/master/scripts/install.ps1 | iex
```

The script installs `srz.exe` to `$env:USERPROFILE\.local\bin` and automatically adds it to your user `PATH`.

---

## 2. Homebrew (macOS & Linux)

The easiest package manager installation on macOS (Apple Silicon) and Linux is via the official tap:

```bash
brew install NewGuy012/tap/srz
```

To update in the future:

```bash
brew update && brew upgrade srz
```

---

## 3. Build from Source (Cargo)

If you have Rust and Cargo installed:

```bash
# Install latest release from crates.io
cargo install srz

# Or clone and build from local source
git clone https://github.com/NewGuy012/srz.git
cd srz
cargo install --path .
```

*Note: Intel Macs (x86_64) should use Cargo source installation.*

---

## 4. Prebuilt Binaries (GitHub Releases)

Download precompiled binaries and checksums directly from the [GitHub Releases Page](https://github.com/NewGuy012/srz/releases):

| Platform | Architecture | Archive |
| :--- | :--- | :--- |
| **macOS** | Apple Silicon (`aarch64`) | `srz-v*-aarch64-apple-darwin.tar.gz` |
| **Linux** | 64-bit x86 (`x86_64`) | `srz-v*-x86_64-unknown-linux-gnu.tar.gz` |
| **Linux** | ARM64 (`aarch64`) | `srz-v*-aarch64-unknown-linux-gnu.tar.gz` |
| **Windows** | 64-bit x86 (`x86_64`) | `srz-v*-x86_64-pc-windows-msvc.zip` |

### Verifying SLSA Provenance

Every release includes SLSA Build Level 3 cryptographic provenance. You can verify that binaries were compiled directly inside the official GitHub Actions runner using the GitHub CLI:

```bash
gh attestation verify srz-v0.7.9-aarch64-apple-darwin.tar.gz --repo NewGuy012/srz
```

*Tip: Downloaded release archives bundle pre-generated shell completions in the `completions/` directory (`srz.bash`, `_srz`, `srz.fish`, `_srz.ps1`).*

---

## 5. In-Place Self Upgrades (`srz upgrade`)

Once installed, `srz` can inspect GitHub Releases, verify SHA-256 checksums, and atomically replace the binary in place:

```bash
# Check if an update is available without installing
srz upgrade --check

# Upgrade to the latest release
srz upgrade

# Force reinstall or target a specific version
srz upgrade --force
srz upgrade --version 0.7.9
```

---

## 6. Shell Completions (`srz completions`)

Generate automatic command-line completions for your active shell:

```bash
# Bash (add to ~/.bashrc)
eval "$(srz completions bash)"

# Zsh (add to ~/.zshrc)
eval "$(srz completions zsh)"

# Fish (add to ~/.config/fish/config.fish)
srz completions fish | source

# PowerShell
srz completions powershell | Out-String | Invoke-Expression
```
