# Configures a drive for testing in CI.
# Credits to astral-sh/uv: https://github.com/astral-sh/uv/blob/d2b9ffdc9e3f336e46b0af18a8554de560bfbefc/.github/workflows/setup-dev-drive.ps1

# When not using a GitHub Actions "larger runner", the `D:` drive is present and
# has similar or better performance characteristics than a ReFS dev drive.
# Sometimes using a larger runner is still more performant (e.g., when running
# the test suite) and we need to create a dev drive. This script automatically
# configures the appropriate drive.
if (Test-Path "D:\") {
    Write-Output "Using existing drive at D:"
    $Drive = "D:"
} else {
	# The size (20 GB) is chosen empirically to be large enough for our
	# workflows; larger drives can take longer to set up.
	$Volume = New-VHD -Path C:/pixi_dev_drive.vhdx -SizeBytes 20GB |
						Mount-VHD -Passthru |
						Initialize-Disk -Passthru |
						New-Partition -AssignDriveLetter -UseMaximumSize |
						Format-Volume -DevDrive -Confirm:$false -Force

	$Drive = "$($Volume.DriveLetter):"

	# Set the drive as trusted
	# See https://learn.microsoft.com/en-us/windows/dev-drive/#how-do-i-designate-a-dev-drive-as-trusted
	fsutil devdrv trust $Drive

	# Disable antivirus filtering on dev drives
	# See https://learn.microsoft.com/en-us/windows/dev-drive/#how-do-i-configure-additional-filters-on-dev-drive
	fsutil devdrv enable /disallowAv

	# Remount so the changes take effect
	Dismount-VHD -Path C:/pixi_dev_drive.vhdx
	Mount-VHD -Path C:/pixi_dev_drive.vhdx

	# Show some debug information
	Write-Output $Volume
	fsutil devdrv query $Drive

    Write-Output "Using Dev Drive at $Volume"
}

$Tmp = "$($Drive)/pixi-tmp"

# Create the directory ahead of time in an attempt to avoid race-conditions
New-Item $Tmp -ItemType Directory

# Move Cargo to the dev drive
New-Item -Path "$($Drive)/.cargo/bin" -ItemType Directory -Force
if (Test-Path "C:/Users/runneradmin/.cargo") {
    Copy-Item -Path "C:/Users/runneradmin/.cargo/*" -Destination "$($Drive)/.cargo/" -Recurse -Force
}

# Set environment variables for GitHub Actions
Write-Output `
    "DEV_DRIVE=$($Drive)" `
    "TMP=$($Tmp)" `
    "TEMP=$($Tmp)" `
    "RUSTUP_HOME=$($Drive)/.rustup" `
    "CARGO_HOME=$($Drive)/.cargo" `
	"RATTLER_CACHE_DIR=$($Drive)/rattler-cache" `
    "PIXI_HOME=$($Drive)/.pixi" `
    "PIXI_WORKSPACE=$($Drive)/pixi" `
    >> $env:GITHUB_ENV
