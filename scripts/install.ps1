$ErrorActionPreference = "Stop"

$Repo = "matthewyjiang/rho"
$BinName = "rho.exe"
$Version = if ($env:RHO_VERSION) { $env:RHO_VERSION } else { "latest" }
$IsWindowsHost = if (Get-Variable -Name IsWindows -ErrorAction SilentlyContinue) { $IsWindows } else { $true }
if (-not $IsWindowsHost) {
    throw "install.ps1 only supports Windows. On Linux or macOS, use scripts/install.sh instead."
}
$InstallDir = if ($env:RHO_INSTALL_DIR) { $env:RHO_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\rho\bin" }
$Target = "x86_64-pc-windows-msvc"
$Asset = "rho-$Target.zip"

function Invoke-Download($Uri, $OutFile, $Required) {
    try {
        Invoke-WebRequest -Uri $Uri -OutFile $OutFile
        return $true
    } catch {
        if ($Required) {
            throw "failed to download $Uri. Check that the release exists and includes $Asset. Original error: $($_.Exception.Message)"
        }
        return $false
    }
}

function Invoke-GitHubApi($Uri) {
    try {
        return Invoke-RestMethod -Uri $Uri -Headers @{ "User-Agent" = "rho-installer" }
    } catch {
        throw "failed to query $Uri. Original error: $($_.Exception.Message)"
    }
}

function Get-ReleaseTag {
    if ($Version -eq "latest") {
        $Release = Invoke-GitHubApi "https://api.github.com/repos/$Repo/releases/latest"
        if ([string]::IsNullOrWhiteSpace($Release.tag_name)) {
            throw "failed to determine latest release tag from GitHub API"
        }
        return $Release.tag_name
    }
    if ($Version -like "rho-coding-agent-*") {
        return $Version
    }
    if ($Version -match "^\d+\.\d+\.\d+([+-].*)?$") {
        return "rho-coding-agent-v$Version"
    }
    return "rho-coding-agent-$Version"
}

function Get-AssetUrl {
    $ReleaseTag = Get-ReleaseTag
    return "https://github.com/$Repo/releases/download/$ReleaseTag/$Asset"
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

function ConvertTo-SingleQuotedLiteral($Value) {
    $Escaped = [string]$Value -replace "'", "''"
    return "'$Escaped'"
}

function Start-DeferredCopy($Source, $Destination, $TempDir, $ParentPid) {
    $Helper = Join-Path $TempDir "rho-complete-update.ps1"
    $HelperContent = @"
`$ErrorActionPreference = "Stop"
`$Source = $(ConvertTo-SingleQuotedLiteral $Source)
`$Destination = $(ConvertTo-SingleQuotedLiteral $Destination)
`$TempDir = $(ConvertTo-SingleQuotedLiteral $TempDir)
`$ParentPid = $ParentPid
try {
    try {
        `$Process = Get-Process -Id `$ParentPid -ErrorAction SilentlyContinue
        if (`$Process) {
            Wait-Process -Id `$ParentPid -ErrorAction SilentlyContinue
        }
    } catch {}

    `$DestinationDir = Split-Path -Parent `$Destination
    New-Item -ItemType Directory -Force -Path `$DestinationDir | Out-Null
    Copy-Item -Path `$Source -Destination `$Destination -Force
} finally {
    Remove-Item -Recurse -Force `$TempDir -ErrorAction SilentlyContinue
}
"@
    Set-Content -Path $Helper -Value $HelperContent -Encoding UTF8
    Start-Process -FilePath "powershell" -ArgumentList @(
        "-NoProfile",
        "-File", "`"$Helper`""
    ) | Out-Null
}

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "rho prebuilt binaries currently require 64-bit Windows. Install with Cargo instead: cargo install rho-coding-agent"
}

$Url = Get-AssetUrl
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) "rho-install-$([System.Guid]::NewGuid())"
$Archive = Join-Path $TempDir $Asset
$Checksum = "$Archive.sha256"
$DeferredCopyStarted = $false

New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
try {
    Write-Host "downloading rho for $Target..."
    Invoke-Download $Url $Archive $true | Out-Null

    try {
        if (-not (Invoke-Download "$Url.sha256" $Checksum $false)) {
            throw "checksum file is unavailable"
        }
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
    $Source = Join-Path $TempDir $BinName
    $Destination = Join-Path $InstallDir $BinName

    if ($env:RHO_UPDATE_PARENT_PID) {
        $ParentPid = 0
        if ([int]::TryParse($env:RHO_UPDATE_PARENT_PID, [ref]$ParentPid) -and $ParentPid -gt 0) {
            Start-DeferredCopy $Source $Destination $TempDir $ParentPid
            $DeferredCopyStarted = $true
        } else {
            throw "RHO_UPDATE_PARENT_PID is not a valid process id: $env:RHO_UPDATE_PARENT_PID"
        }
    } else {
        Copy-Item -Path $Source -Destination $Destination -Force
    }

    $PathChanged = Add-ToUserPath $InstallDir
    if ($DeferredCopyStarted) {
        Write-Host "rho update staged for $Destination. It will finish after the current rho process exits."
    } else {
        Write-Host "rho installed to $Destination"
    }
    if ($PathChanged) {
        Write-Host "added $InstallDir to your user PATH. restart your terminal if rho is not found."
    }
} finally {
    if (-not $DeferredCopyStarted) {
        Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
    }
}
