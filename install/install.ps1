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
.LINK
    https://pixi.sh
.LINK
    https://github.com/prefix-dev/pixi
#>
param (
    [string] $PixiVersion = 'latest',
    [string] $PixiHome = "$Env:USERPROFILE\.pixi"
)

Set-StrictMode -Version Latest

if ($Env:PIXI_VERSION) {
    $PixiVersion = $Env:PIXI_VERSION
}

if ($Env:PIXI_HOME) {
    $PixiHome = $Env:PIXI_HOME
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
        New-Item -ItemType directory -Path $BinDir
    }

    $ZIP_FILE = $TEMP_FILE + ".zip"
    Rename-Item -Path $TEMP_FILE -NewName $ZIP_FILE

    # Extract pixi from the downloaded zip file
    Expand-Archive -Path $ZIP_FILE -DestinationPath $BinDir -Force

    # Add pixi to PATH if the folder is not already in the PATH variable
    $PATH = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($PATH -notlike "*$BinDir*") {
        Write-Output "Adding $BinDir to PATH`n"
        [Environment]::SetEnvironmentVariable("Path", "$BinDir;" + [Environment]::GetEnvironmentVariable("Path", "User"), "User")
    } else {
        Write-Output "$BinDir is already in PATH`n"
    }
} catch {
    Write-Host "Error: '$DOWNLOAD_URL' is not available or failed to download"
    exit 1
} finally {
    Remove-Item -Path $ZIP_FILE
}
