# srz (Summarize CLI) installer for Windows PowerShell
# Usage:
#   irm https://raw.githubusercontent.com/NewGuy012/srz/master/install.ps1 | iex

$ErrorActionPreference = 'Stop'

$Repo = "NewGuy012/srz"
$Target = "x86_64-pc-windows-msvc"
$InstallDir = if ($env:INSTALL_DIR) { $env:INSTALL_DIR } else { Join-Path $env:USERPROFILE ".local\bin" }

# Ensure TLS 1.2 is active for web downloads
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

# Determine download URL
if ($env:VERSION) {
    $ArchiveName = "srz-$($env:VERSION)-$Target.zip"
    $DownloadUrl = "https://github.com/$Repo/releases/download/$($env:VERSION)/$ArchiveName"
} else {
    $ArchiveName = "srz-$Target.zip"
    $DownloadUrl = "https://github.com/$Repo/releases/latest/download/$ArchiveName"
}

Write-Host "Installing srz for $Target..." -ForegroundColor Cyan

$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("srz-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

try {
    $ZipPath = Join-Path $TempDir $ArchiveName
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $ZipPath -UseBasicParsing

    Expand-Archive -Path $ZipPath -DestinationPath $TempDir -Force

    if (-not (Test-Path -Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    $ExeSource = Join-Path $TempDir "srz.exe"
    $ExeDest = Join-Path $InstallDir "srz.exe"

    Copy-Item -Path $ExeSource -Destination $ExeDest -Force

    Write-Host "✔ Successfully installed srz to $ExeDest" -ForegroundColor Green

    # Check and update User PATH if not present
    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($null -eq $UserPath) { $UserPath = "" }

    $PathEntries = $UserPath -split ';' | Where-Object { $_ -ne "" }
    if ($PathEntries -notcontains $InstallDir) {
        $NewUserPath = if ($UserPath -eq "") { $InstallDir } else { "$UserPath;$InstallDir" }
        [Environment]::SetEnvironmentVariable("Path", $NewUserPath, "User")
        $env:Path = "$env:Path;$InstallDir"
        Write-Host "✔ Added $InstallDir to your User PATH." -ForegroundColor Green
        Write-Host "Restart your terminal if 'srz' is not recognized immediately." -ForegroundColor Yellow
    }
}
catch {
    Write-Error "Failed to install srz: $_"
    exit 1
}
finally {
    if (Test-Path -Path $TempDir) {
        Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
