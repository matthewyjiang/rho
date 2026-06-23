$ErrorActionPreference = "Stop"

$Repo = "matthewyjiang/rho"
$BinName = "rho.exe"
$Version = if ($env:RHO_VERSION) { $env:RHO_VERSION } else { "latest" }
$InstallDir = if ($env:RHO_INSTALL_DIR) { $env:RHO_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\rho\bin" }
$Target = "x86_64-pc-windows-msvc"
$Asset = "rho-$Target.zip"

function Get-AssetUrl {
    if ($Version -eq "latest") {
        return "https://github.com/$Repo/releases/latest/download/$Asset"
    }
    return "https://github.com/$Repo/releases/download/$Version/$Asset"
}

function Add-ToUserPath($Dir) {
    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $Parts = @()
    if ($UserPath) {
        $Parts = $UserPath -split ";" | Where-Object { $_ }
    }

    if ($Parts -contains $Dir) {
        return $false
    }

    $NewPath = if ($UserPath) { "$UserPath;$Dir" } else { $Dir }
    [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
    $env:Path = "$env:Path;$Dir"
    return $true
}

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "rho prebuilt binaries currently require 64-bit Windows. Install with Cargo instead: cargo install rho-coding-agent"
}

$Url = Get-AssetUrl
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) "rho-install-$([System.Guid]::NewGuid())"
$Archive = Join-Path $TempDir $Asset
$Checksum = "$Archive.sha256"

New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
try {
    Write-Host "downloading rho for $Target..."
    Invoke-WebRequest -Uri $Url -OutFile $Archive

    try {
        Invoke-WebRequest -Uri "$Url.sha256" -OutFile $Checksum
        $Expected = ((Get-Content $Checksum -Raw) -split "\s+")[0].Trim().ToLowerInvariant()
        $Actual = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
        if ($Actual -ne $Expected) {
            throw "checksum verification failed"
        }
    } catch {
        if ($_.Exception.Message -eq "checksum verification failed") {
            throw
        }
        Write-Warning "checksum file is unavailable, skipping verification"
    }

    Expand-Archive -Path $Archive -DestinationPath $TempDir -Force
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Path (Join-Path $TempDir $BinName) -Destination (Join-Path $InstallDir $BinName) -Force

    $PathChanged = Add-ToUserPath $InstallDir
    Write-Host "rho installed to $(Join-Path $InstallDir $BinName)"
    if ($PathChanged) {
        Write-Host "added $InstallDir to your user PATH. restart your terminal if rho is not found."
    }
} finally {
    Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
}
