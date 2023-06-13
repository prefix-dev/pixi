param (
    [string]$PIXI_VERSION = "latest",
    [string]$PIXI_DIR = "$HOME/.pixi/bin"
)

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

    # Extract pixi from the downloaded zip file
    Expand-Archive -Path $TEMP_FILE -DestinationPath $PIXI_DIR -Force

    Write-Host "Installation complete. Please add '$PIXI_DIR' to your PATH."

} catch {
    Write-Host "Error: '$DOWNLOAD_URL' is not available or failed to download"
    exit 1
} finally {
    Remove-Item -Path $TEMP_FILE
}
