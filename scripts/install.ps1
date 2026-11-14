# srz (Summarize CLI) installer for Windows
# Usage (PowerShell or CMD):
#   powershell -ExecutionPolicy ByPass -c "irm https://raw.githubusercontent.com/NewGuy012/srz/master/scripts/install.ps1 | iex"

& {
    $prevErrorAction = $ErrorActionPreference
    $prevProgress = $ProgressPreference
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    $TempDir = $null

    try {
        $Repo = "NewGuy012/srz"
        $Target = "x86_64-pc-windows-msvc"
        $InstallDir = if ($env:INSTALL_DIR) { $env:INSTALL_DIR } else { Join-Path $env:USERPROFILE ".local\bin" }

        # Ensure TLS 1.2 is enabled without disabling newer protocols like TLS 1.3
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

        # Architecture check
        if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") {
            Write-Host "Notice: Detected Windows ARM64; installing x86_64 binary via Windows Prism emulation." -ForegroundColor Cyan
        } elseif ($env:PROCESSOR_ARCHITECTURE -eq "x86" -and -not $env:PROCESSOR_ARCHITEW6432) {
            throw "32-bit Windows is not supported with prebuilt binaries. Please install from source using 'cargo install srz'."
        }

        # Determine download URL (case-sensitive version validation matching install.sh)
        if ($env:VERSION) {
            if ($env:VERSION -cnotmatch '^v?[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$') {
                throw "Invalid version format '$($env:VERSION)'. Must match semver (e.g. '0.4.0' or 'v0.4.0')."
            }
            $Tag = if ($env:VERSION.StartsWith("v")) { $env:VERSION } else { "v$($env:VERSION)" }
            $ArchiveName = "srz-$Tag-$Target.zip"
            $DownloadUrl = "https://github.com/$Repo/releases/download/$Tag/$ArchiveName"
        } else {
            $ArchiveName = "srz-$Target.zip"
            $DownloadUrl = "https://github.com/$Repo/releases/latest/download/$ArchiveName"
        }

        Write-Host "Installing srz for $Target..." -ForegroundColor Cyan

        $TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("srz-install-" + [System.Guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

        $ZipPath = Join-Path $TempDir $ArchiveName
        Invoke-WebRequest -Uri $DownloadUrl -OutFile $ZipPath -UseBasicParsing

        # Download and parse SHA256 checksum; FORCE=1 only bypasses missing/unparseable checksum files
        $ShaUrl = "$DownloadUrl.sha256"
        $ShaPath = "$ZipPath.sha256"
        $ExpectedHash = $null
        try {
            Invoke-WebRequest -Uri $ShaUrl -OutFile $ShaPath -UseBasicParsing
            $ShaContent = ([string](Get-Content -Path $ShaPath -Raw)).Trim()
            $CandidateHash = ($ShaContent -split '\s+')[0]
            if ($CandidateHash -match '^[0-9a-fA-F]{64}$') {
                $ExpectedHash = $CandidateHash.ToLowerInvariant()
            } else {
                throw "Checksum file does not contain a valid 64-character SHA256 hash."
            }
        } catch {
            if ($env:FORCE -eq "1") {
                Write-Warning "Could not retrieve release checksum ($_); bypassing due to FORCE=1."
            } else {
                throw "Failed to retrieve SHA256 checksum: $_. Run with `$env:FORCE='1' to bypass."
            }
        }

        # Never allow FORCE=1 to bypass an explicit SHA256 mismatch
        if ($null -ne $ExpectedHash) {
            Write-Host "Verifying SHA256 checksum..." -ForegroundColor Cyan
            $ComputedHash = (Get-FileHash -Path $ZipPath -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($ComputedHash -ne $ExpectedHash) {
                throw "SHA256 checksum mismatch! Expected: $ExpectedHash, Computed: $ComputedHash"
            }
            Write-Host "$([char]0x2714) Checksum verified successfully." -ForegroundColor Green
        }

        $ExtractDir = Join-Path $TempDir "unpacked"
        Expand-Archive -Path $ZipPath -DestinationPath $ExtractDir -Force

        $ExeSource = Join-Path $ExtractDir "srz.exe"
        if (-not (Test-Path -Path $ExeSource)) {
            throw "srz.exe was not found in the extracted archive."
        }

        if (-not (Test-Path -Path $InstallDir)) {
            New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        }

        $ExeDest = Join-Path $InstallDir "srz.exe"

        # Handle active process replacement (rename locked binary on Windows with rollback on copy failure)
        if (Test-Path -Path $ExeDest) {
            try {
                Copy-Item -Path $ExeSource -Destination $ExeDest -Force
            } catch {
                $OldBackup = Join-Path $InstallDir ("srz.old." + [System.Guid]::NewGuid().ToString("N") + ".exe")
                Move-Item -Path $ExeDest -Destination $OldBackup -Force
                try {
                    Copy-Item -Path $ExeSource -Destination $ExeDest -Force
                } catch {
                    if (Test-Path -Path $OldBackup) {
                        Move-Item -Path $OldBackup -Destination $ExeDest -Force -ErrorAction SilentlyContinue
                    }
                    throw
                }
                Remove-Item -Path $OldBackup -Force -ErrorAction SilentlyContinue
            }
        } else {
            Copy-Item -Path $ExeSource -Destination $ExeDest -Force
        }

        Write-Host "$([char]0x2714) Successfully installed srz to $ExeDest" -ForegroundColor Green

        # Check and update User PATH if not present
        $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
        if ($null -eq $UserPath) { $UserPath = "" }

        $NormalizedInstallDir = $InstallDir.Replace('/', '\').TrimEnd('\')
        $PathEntries = $UserPath -split ';' | Where-Object { $_ -ne "" } | ForEach-Object { $_.Replace('/', '\').TrimEnd('\') }

        if ($PathEntries -notcontains $NormalizedInstallDir) {
            $NewUserPath = if ($UserPath -eq "") { $NormalizedInstallDir } else { "$UserPath;$NormalizedInstallDir" }
            [Environment]::SetEnvironmentVariable("Path", $NewUserPath, "User")
            $env:Path = "$env:Path;$NormalizedInstallDir"
            Write-Host "$([char]0x2714) Added $NormalizedInstallDir to your User PATH." -ForegroundColor Green
            Write-Host "Restart your terminal if 'srz' is not recognized immediately." -ForegroundColor Yellow

            # Broadcast WM_SETTINGCHANGE to notify running Explorer and terminal sessions
            Add-Type -Namespace Win32 -Name NativeMethods -MemberDefinition @"
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
"@ -ErrorAction SilentlyContinue
            try {
                $result = [UIntPtr]::Zero
                [Win32.NativeMethods]::SendMessageTimeout([IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, "Environment", 2, 5000, [ref]$result) | Out-Null
            } catch {
                Write-Verbose "WM_SETTINGCHANGE broadcast skipped: $_"
            }
        }
    }
    catch {
        throw "Failed to install srz: $_"
    }
    finally {
        $ErrorActionPreference = $prevErrorAction
        $ProgressPreference = $prevProgress
        if ($TempDir -and (Test-Path -Path $TempDir)) {
            Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
