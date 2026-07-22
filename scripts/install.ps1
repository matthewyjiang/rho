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

function Test-ReleaseHasAssets($Release) {
    return $Release.assets.name -contains $Asset -and $Release.assets.name -contains "$Asset.sha256"
}

function Get-ReleaseForAsset {
    if ($Version -eq "latest") {
        $Latest = Invoke-GitHubApi "https://api.github.com/repos/$Repo/releases/latest"
        if ([string]::IsNullOrWhiteSpace($Latest.tag_name)) {
            throw "failed to determine latest release tag from GitHub API"
        }
        if (Test-ReleaseHasAssets $Latest) {
            return $Latest
        }

        $Fallback = Invoke-GitHubApi "https://api.github.com/repos/$Repo/releases?per_page=100" |
            Where-Object { Test-ReleaseHasAssets $_ } |
            Select-Object -First 1
        if (-not $Fallback) {
            throw "$($Latest.tag_name) is tagged but required assets $Asset and $Asset.sha256 are not both published yet, and no earlier compatible release was found. Install from source instead: cargo install rho-coding-agent"
        }
        Write-Warning "$($Latest.tag_name) is tagged but required assets are not both published yet; installing $($Fallback.tag_name) instead"
        return $Fallback
    }

    if ($Version -like "rho-coding-agent-*") {
        $ReleaseTag = $Version
    } elseif ($Version -match "^\d+\.\d+\.\d+([+-].*)?$") {
        $ReleaseTag = "rho-coding-agent-v$Version"
    } else {
        $ReleaseTag = "rho-coding-agent-$Version"
    }
    $Release = Invoke-GitHubApi "https://api.github.com/repos/$Repo/releases/tags/$ReleaseTag"
    if (-not (Test-ReleaseHasAssets $Release)) {
        throw "release $ReleaseTag does not include both $Asset and $Asset.sha256. Install from source instead: cargo install rho-coding-agent"
    }
    return $Release
}

function Get-AssetUrl {
    $Release = Get-ReleaseForAsset
    return ($Release.assets | Where-Object { $_.name -eq $Asset } | Select-Object -First 1).browser_download_url
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

function Start-DeferredCopy($Source, $Destination, $TempDir, $ParentPid, $CredentialBackend) {
    $Helper = Join-Path $TempDir "rho-complete-update.ps1"
    $HelperContent = @"
`$ErrorActionPreference = "Stop"
`$Source = $(ConvertTo-SingleQuotedLiteral $Source)
`$Destination = $(ConvertTo-SingleQuotedLiteral $Destination)
`$TempDir = $(ConvertTo-SingleQuotedLiteral $TempDir)
`$ParentPid = $ParentPid
`$CredentialBackend = $(ConvertTo-SingleQuotedLiteral $CredentialBackend)
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
    if (`$CredentialBackend) {
        if (`$CredentialBackend -eq "file") {
            & `$Destination credential-store probe file
            if (`$LASTEXITCODE -ne 0) { throw "local file credential storage is unavailable" }
        }
        & `$Destination credential-store set `$CredentialBackend
        if (`$LASTEXITCODE -ne 0) { throw "failed to set credential store to `$CredentialBackend" }
    }
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

function Set-CredentialStorePreference($RhoPath) {
    function Set-FileCredentialBackend {
        Write-Host "File storage uses a private user-only ACL but is not encrypted at rest."
        & $RhoPath credential-store probe file
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "local file credential storage is unavailable; leaving the OS default"
            return $false
        }
        & $RhoPath credential-store set file
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "failed to set credential store to file; leaving the OS default"
            return $false
        }
        return $true
    }

    function Set-OsCredentialBackend {
        & $RhoPath credential-store set os
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "failed to set credential store to os; leaving the OS default"
            return $false
        }
        return $true
    }

    function Read-YesNo($Prompt) {
        try {
            $Answer = Read-Host $Prompt
        } catch {
            Write-Warning "could not read credential-store choice; leaving the OS default"
            return $null
        }
        if ([string]::IsNullOrWhiteSpace($Answer) -or $Answer -match "^(y|yes)$") {
            return $true
        }
        if ($Answer -match "^(n|no)$") {
            return $false
        }
        Write-Warning "unrecognized choice; leaving the OS default"
        return $null
    }

    if ($env:RHO_CREDENTIAL_STORE) {
        if ($env:RHO_CREDENTIAL_STORE -eq "file") {
            & $RhoPath credential-store probe file
            if ($LASTEXITCODE -ne 0) {
                throw "local file credential storage is unavailable"
            }
        }
        & $RhoPath credential-store set $env:RHO_CREDENTIAL_STORE
        if ($LASTEXITCODE -ne 0) {
            throw "failed to set credential store to $env:RHO_CREDENTIAL_STORE"
        }
        return
    }

    $Status = (& $RhoPath credential-store status 2>$null | Out-String).Trim()
    if ($Status -eq "os" -or $Status -eq "file") {
        Write-Host "credential-store choice already configured ($Status); keeping it"
        return
    }

    if (
        $env:CI -or
        $env:RHO_INSTALL_NONINTERACTIVE -or
        [Console]::IsInputRedirected -or
        [Console]::IsOutputRedirected
    ) {
        Write-Host "note: credential store left unset (OS default)."
        Write-Host "      run '$RhoPath credential-store probe os' to check OS credential storage"
        Write-Host "      use '$RhoPath credential-store set file' for private local file storage"
        return
    }

    & $RhoPath credential-store probe os *> $null
    $OsAvailable = $LASTEXITCODE -eq 0
    if ($OsAvailable) {
        $UseOs = Read-YesNo "OS credential store is available and recommended. Use it? [Y/n]"
        if ($UseOs -eq $true) {
            [void](Set-OsCredentialBackend)
        } elseif ($UseOs -eq $false) {
            [void](Set-FileCredentialBackend)
        }
        return
    }

    Write-Host "No usable OS credential store was found in this session."
    $UseFile = Read-YesNo "Use local file credential storage? [Y/n]"
    if ($UseFile -eq $true) {
        [void](Set-FileCredentialBackend)
    } elseif ($UseFile -eq $false) {
        Write-Warning "leaving the OS default; configure the OS credential store before login"
    }
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

    if (-not (Invoke-Download "$Url.sha256" $Checksum $false)) {
        throw "failed to download required checksum: $Url.sha256"
    }
    $Expected = ((Get-Content $Checksum -Raw) -split "\s+")[0].Trim().ToLowerInvariant()
    $Actual = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
    if ([string]::IsNullOrWhiteSpace($Expected) -or $Actual -ne $Expected) {
        throw "checksum verification failed for $Url"
    }

    Expand-Archive -Path $Archive -DestinationPath $TempDir -Force
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $Source = Join-Path $TempDir $BinName
    $Destination = Join-Path $InstallDir $BinName

    if ($env:RHO_UPDATE_PARENT_PID) {
        $ParentPid = 0
        if ([int]::TryParse($env:RHO_UPDATE_PARENT_PID, [ref]$ParentPid) -and $ParentPid -gt 0) {
            Start-DeferredCopy $Source $Destination $TempDir $ParentPid $env:RHO_CREDENTIAL_STORE
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
        Set-CredentialStorePreference $Destination
    }
    if ($PathChanged) {
        Write-Host "added $InstallDir to your user PATH. restart your terminal if rho is not found."
    }
} finally {
    if (-not $DeferredCopyStarted) {
        Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
    }
}
