# /// conda-script
# channels = ["conda-forge"]
# entrypoint = "pwsh -NoProfile -File ${SCRIPT}"
#
# [dependencies]
# powershell = "*"
# /// end-conda-script
if (-not (Get-Module -ListAvailable -Name powershell-yaml)) {
    Install-Module -Name powershell-yaml -Scope CurrentUser -Force
}
Import-Module powershell-yaml

$document = ConvertFrom-Yaml -Ordered @"
name: conda-script
languages:
  - python
  - powershell
"@
$document.count = $document.languages.Count
ConvertTo-Yaml $document
