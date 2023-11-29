param (
    [string]$PIXI_DIR = "$Env:USERPROFILE\.pixi\bin"
)

if ($Env:PIXI_VERSION) {
    $PIXI_VERSION = $Env:PIXI_VERSION
} else {
    $PIXI_VERSION = "latest"
}

# Repository name
$REPO = "prefix-dev/pixi"
$ARCH = "x86_64"
$PLATFORM = "pc-windows-msvc"

$BINARY = "pixi-$ARCH-$PLATFORM"

if ($PIXI_VERSION -eq "latest") {
    $DOWNLOAD_URL = "https://github.com/$REPO/releases/latest/download/$BINARY.zip"
} else {
    $DOWNLOAD_URL = "https://github.com/$REPO/releases/download/$PIXI_VERSION/$BINARY.zip"
}

Write-Host "This script will automatically download and install Pixi ($PIXI_VERSION) for you."
Write-Host "Getting it from this url: $DOWNLOAD_URL"
Write-Host "The binary will be installed into '$PIXI_DIR'"

$TEMP_FILE = [System.IO.Path]::GetTempFileName()

try {
    Invoke-WebRequest -Uri $DOWNLOAD_URL -OutFile $TEMP_FILE

    # Create the install dir if it doesn't exist
    if (!(Test-Path -Path $PIXI_DIR )) {
        New-Item -ItemType directory -Path $PIXI_DIR
    }

    $ZIP_FILE = $TEMP_FILE + ".zip"
    Rename-Item -Path $TEMP_FILE -NewName $ZIP_FILE

    # Extract pixi from the downloaded zip file
    Expand-Archive -Path $ZIP_FILE -DestinationPath $PIXI_DIR -Force

    # Add pixi to PATH if the folder is not already in the PATH variable
    $PATH = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($PATH -notlike "*$PIXI_DIR*") {
        Write-Output "Adding $PIXI_DIR to PATH`n"
        [Environment]::SetEnvironmentVariable("Path", "$PIXI_DIR;" + [Environment]::GetEnvironmentVariable("Path", "User"), "User")
    } else {
        Write-Output "$PIXI_DIR is already in PATH`n"
    }
} catch {
    Write-Host "Error: '$DOWNLOAD_URL' is not available or failed to download"
    exit 1
} finally {
    Remove-Item -Path $ZIP_FILE
}
