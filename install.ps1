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
.LINK
    https://pixi.sh
.LINK
    https://github.com/prefix-dev/pixi
#>
param (
    [string] $PixiVersion = 'latest',
    [string] $PixiHome = "$Env:USERPROFILE\.pixi",
    [switch] $NoPathUpdate
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

if ($Env:PIXI_VERSION) {
    $PixiVersion = $Env:PIXI_VERSION
}

if ($Env:PIXI_HOME) {
    $PixiHome = $Env:PIXI_HOME
}

if ($Env:PIXI_NO_PATH_UPDATE) {
    $NoPathUpdate = $true
}

# Repository name
$REPO = 'prefix-dev/pixi'
$ARCH = 'x86_64'
$PLATFORM = 'pc-windows-msvc'

$BINARY = "pixi-$ARCH-$PLATFORM"

if ($PixiVersion -eq 'latest') {
    $DOWNLOAD_URL = "https://github.com/$REPO/releases/latest/download/$BINARY.zip"
} else {
    $DOWNLOAD_URL = "https://github.com/$REPO/releases/download/$PixiVersion/$BINARY.zip"
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
