# This creates a 20GB dev drive, and exports all required environment
# variables so that rustup, pixi and others all use the dev drive as much
# as possible.
$Volume = New-VHD -Path C:/pixi_dev_drive.vhdx -SizeBytes 20GB |
        Mount-VHD -Passthru |
        Initialize-Disk -Passthru |
        New-Partition -AssignDriveLetter -UseMaximumSize |
        Format-Volume -FileSystem NTFS -Confirm:$false -Force

Write-Output $Volume

$Drive = "$($Volume.DriveLetter):"
$Tmp = "$($Drive)/pixi-tmp"

# Create the directory ahead of time in an attempt to avoid race-conditions
New-Item $Tmp -ItemType Directory

Write-Output `
	"DEV_DRIVE=$($Drive)" `
	"TMP=$($Tmp)" `
	"TEMP=$($Tmp)" `
	"RATTLER_CACHE_DIR=$($Drive)/rattler-cache" `
	"RUSTUP_HOME=$($Drive)/.rustup" `
	"CARGO_HOME=$($Drive)/.cargo" `
	"PIXI_HOME=$($Drive)/.pixi" `
	"PIXI_WORKSPACE=$($Drive)/pixi" `
	>> $env:GITHUB_ENV
