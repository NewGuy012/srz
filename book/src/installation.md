# Installation

`srz` provides multiple installation methods across macOS, Linux, and Windows.

---

## 1. Homebrew (macOS & Linux)

The easiest method on macOS (Apple Silicon) and Linux is via the official Homebrew tap:

```bash
brew tap NewGuy012/tap
brew install srz
```

To update in the future:
```bash
brew update && brew upgrade srz
```

---

## 2. Automated One-Line Install Script

### macOS & Linux (curl + sh)

```bash
curl -fsSL https://raw.githubusercontent.com/NewGuy012/srz/master/scripts/install.sh | sh
```

By default, the binary is installed to `~/.local/bin/srz`. You can customize the install directory:
```bash
curl -fsSL https://raw.githubusercontent.com/NewGuy012/srz/master/scripts/install.sh | INSTALL_DIR=/usr/local/bin sh
```

### Windows (PowerShell)

Run the following in PowerShell (as Administrator or regular user):

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://raw.githubusercontent.com/NewGuy012/srz/master/scripts/install.ps1 | iex"
```

The script installs `srz.exe` to `$env:USERPROFILE\.local\bin` and automatically adds it to your user `PATH`.

---

## 3. Install via Cargo (crates.io)

If you have the Rust toolchain installed:

```bash
cargo install srz
```

---

## 4. Prebuilt Binaries (GitHub Releases)

Download precompiled, cryptographically attested binaries from the [GitHub Releases Page](https://github.com/NewGuy012/srz/releases):

* **macOS (Apple Silicon)**: `srz-v*-aarch64-apple-darwin.tar.gz`
* **Linux (x86_64)**: `srz-v*-x86_64-unknown-linux-gnu.tar.gz`
* **Linux (ARM64)**: `srz-v*-aarch64-unknown-linux-gnu.tar.gz`
* **Windows (x64)**: `srz-v*-x86_64-pc-windows-msvc.zip`

### Verifying SLSA Provenance

You can verify that the downloaded binary was compiled directly inside the official GitHub Actions runner using the GitHub CLI:

```bash
gh attestation verify srz-v0.5.0-x86_64-unknown-linux-gnu.tar.gz --repo NewGuy012/srz
```

---

## 5. In-Place Self Upgrades

Once installed, `srz` can upgrade itself to the latest release in place:

```bash
srz upgrade
```
*Checks GitHub Releases, validates SHA-256 checksums, and atomically replaces the binary.*
