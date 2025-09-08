<#
.SYNOPSIS
    Pixi install script.
.DESCRIPTION
    This script is used to install Pixi on Windows from the command line.
.PARAMETER PixiVersion
    Specifies the version of Pixi to install.
    The default value is 'latest'. You can also specify it by setting the
    environment variable 'PIXI_VERSION'.
.PARAMETER PixiHome
    Specifies Pixi's home directory.
    The default value is '$Env:USERPROFILE\.pixi'. You can also specify it by
    setting the environment variable 'PIXI_HOME'.
.PARAMETER NoPathUpdate
    If specified, the script will not update the PATH environment variable.
.PARAMETER PixiRepourl
    Specifies Pixi's repo url.
    The default value is 'https://github.com/prefix-dev/pixi'. You can also specify it by
    setting the environment variable 'PIXI_REPOURL'.
.LINK
    https://pixi.sh
.LINK
    https://github.com/prefix-dev/pixi
.NOTES
    Version: v0.54.2
#>
param (
    [string] $PixiVersion = 'latest',
    [string] $PixiHome = "$Env:USERPROFILE\.pixi",
    [switch] $NoPathUpdate,
    [string] $PixiRepourl = 'https://github.com/prefix-dev/pixi'
)

Set-StrictMode -Version Latest

function Publish-Env {
    if (-not ("Win32.NativeMethods" -as [Type])) {
        Add-Type -Namespace Win32 -Name NativeMethods -MemberDefinition @"
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(
    IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam,
    uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
"@
    }

    $HWND_BROADCAST = [IntPtr] 0xffff
    $WM_SETTINGCHANGE = 0x1a
    $result = [UIntPtr]::Zero

    [Win32.Nativemethods]::SendMessageTimeout($HWND_BROADCAST,
        $WM_SETTINGCHANGE,
        [UIntPtr]::Zero,
        "Environment",
        2,
        5000,
        [ref] $result
    ) | Out-Null
}

function Write-Env {
    param(
        [String] $name,
        [String] $val,
        [Switch] $global
    )

    $RegisterKey = if ($global) {
        Get-Item -Path 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager'
    } else {
        Get-Item -Path 'HKCU:'
    }

    $EnvRegisterKey = $RegisterKey.OpenSubKey('Environment', $true)
    if ($null -eq $val) {
        $EnvRegisterKey.DeleteValue($name)
    } else {
        $RegistryValueKind = if ($val.Contains('%')) {
            [Microsoft.Win32.RegistryValueKind]::ExpandString
        } elseif ($EnvRegisterKey.GetValue($name)) {
            $EnvRegisterKey.GetValueKind($name)
        } else {
            [Microsoft.Win32.RegistryValueKind]::String
        }
        $EnvRegisterKey.SetValue($name, $val, $RegistryValueKind)
    }
    Publish-Env
}

function Get-Env {
    param(
        [String] $name,
        [Switch] $global
    )

    $RegisterKey = if ($global) {
        Get-Item -Path 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager'
    } else {
        Get-Item -Path 'HKCU:'
    }

    $EnvRegisterKey = $RegisterKey.OpenSubKey('Environment')
    $RegistryValueOption = [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
    $EnvRegisterKey.GetValue($name, $null, $RegistryValueOption)
}

function Get-TargetTriple() {
  try {
    # NOTE: this might return X64 on ARM64 Windows, which is OK since emulation is available.
    # It works correctly starting in PowerShell Core 7.3 and Windows PowerShell in Win 11 22H2.
    # Ideally this would just be
    #   [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    # but that gets a type from the wrong assembly on Windows PowerShell (i.e. not Core)
    $a = [System.Reflection.Assembly]::LoadWithPartialName("System.Runtime.InteropServices.RuntimeInformation")
    $t = $a.GetType("System.Runtime.InteropServices.RuntimeInformation")
    $p = $t.GetProperty("OSArchitecture")
    # Possible OSArchitecture Values: https://learn.microsoft.com/dotnet/api/system.runtime.interopservices.architecture
    # Rust supported platforms: https://doc.rust-lang.org/stable/rustc/platform-support.html
    switch ($p.GetValue($null).ToString())
    {
      "X86" { return "i686-pc-windows-msvc" }
      "X64" { return "x86_64-pc-windows-msvc" }
      "Arm" { return "thumbv7a-pc-windows-msvc" }
      "Arm64" { return "aarch64-pc-windows-msvc" }
    }
  } catch {
    # The above was added in .NET 4.7.1, so Windows PowerShell in versions of Windows
    # prior to Windows 10 v1709 may not have this API.
    Write-Verbose "Get-TargetTriple: Exception when trying to determine OS architecture."
    Write-Verbose $_
  }

  # This is available in .NET 4.0. We already checked for PS 5, which requires .NET 4.5.
  Write-Verbose("Get-TargetTriple: falling back to Is64BitOperatingSystem.")
  if ([System.Environment]::Is64BitOperatingSystem) {
    return "x86_64-pc-windows-msvc"
  } else {
    return "i686-pc-windows-msvc"
  }
}

if ($Env:PIXI_VERSION) {
    $PixiVersion = $Env:PIXI_VERSION
}

if ($Env:PIXI_HOME) {
    $PixiHome = $Env:PIXI_HOME
}

if ($Env:PIXI_NO_PATH_UPDATE) {
    $NoPathUpdate = $true
}

if ($Env:PIXI_REPOURL) {
    $PixiRepourl = $Env:PIXI_REPOURL -replace '/$', ''
}

# Repository name
$ARCH = Get-TargetTriple

if (-not @("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc") -contains $ARCH) {
    throw "ERROR: could not find binaries for this platform ($ARCH)."
}

$BINARY = "pixi-$ARCH"

if ($PixiVersion -eq 'latest') {
    $DOWNLOAD_URL = "$PixiRepourl/releases/latest/download/$BINARY.zip"
} else {
    # Check if version is incorrectly specified without prefix 'v', and prepend 'v' in this case
    $PixiVersion = "v" + ($PixiVersion -replace '^v', '')
    $DOWNLOAD_URL = "$PixiRepourl/releases/download/$PixiVersion/$BINARY.zip"
}

$BinDir = Join-Path $PixiHome 'bin'

Write-Host "This script will automatically download and install Pixi ($PixiVersion) for you."
Write-Host "Getting it from this url: $DOWNLOAD_URL"
Write-Host "The binary will be installed into '$BinDir'"

$TEMP_FILE = [System.IO.Path]::GetTempFileName()

try {
    Invoke-WebRequest -Uri $DOWNLOAD_URL -OutFile $TEMP_FILE

    # Create the install dir if it doesn't exist
    if (!(Test-Path -Path $BinDir)) {
        New-Item -ItemType Directory -Path $BinDir | Out-Null
    }

    $ZIP_FILE = $TEMP_FILE + ".zip"
    Rename-Item -Path $TEMP_FILE -NewName $ZIP_FILE

    # Extract pixi from the downloaded zip file
    Expand-Archive -Path $ZIP_FILE -DestinationPath $BinDir -Force
} catch {
    Write-Host "Error: '$DOWNLOAD_URL' is not available or failed to download"
    exit 1
} finally {
    Remove-Item -Path $ZIP_FILE
}

# Add pixi to PATH if the folder is not already in the PATH variable
if (!$NoPathUpdate) {
    $PATH = Get-Env 'PATH'
    if ($PATH -notlike "*$BinDir*") {
        Write-Output "Adding $BinDir to PATH"
        # For future sessions
        Write-Env -name 'PATH' -val "$BinDir;$PATH"
        # For current session
        $Env:PATH = "$BinDir;$PATH"
        Write-Output "You may need to restart your shell"
    } else {
        Write-Output "$BinDir is already in PATH"
    }
} else {
    Write-Output "You may need to update your PATH manually to use pixi"
}
