param(
    [Parameter(Mandatory = $true)]
    [string]$Executable
)

$ErrorActionPreference = 'Stop'
$exePath = (Resolve-Path -LiteralPath $Executable).Path
$manifestPath = Join-Path ([System.IO.Path]::GetTempPath()) "pixi-manifest-$PID.xml"

$mt = (Get-Command mt.exe -ErrorAction SilentlyContinue).Source
if (-not $mt) {
    $mt = Get-ChildItem -Path "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\mt.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $mt) {
    throw 'Windows SDK mt.exe was not found'
}

& $mt -nologo "-inputresource:$exePath;#1" "-out:$manifestPath"
if ($LASTEXITCODE -ne 0) {
    throw "Could not extract the application manifest from $exePath"
}

[xml]$manifest = Get-Content -LiteralPath $manifestPath -Raw
$namespaces = [System.Xml.XmlNamespaceManager]::new($manifest.NameTable)
$namespaces.AddNamespace('ws', 'http://schemas.microsoft.com/SMI/2016/WindowsSettings')
$setting = $manifest.SelectSingleNode('//ws:longPathAware', $namespaces)
if ($null -eq $setting -or $setting.InnerText.Trim() -ne 'true') {
    throw "$exePath is not declared long-path aware"
}
