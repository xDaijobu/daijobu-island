param(
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [string]$SdkVersion = '10.0.26100.0'
)
$ErrorActionPreference = 'Stop'
$sdkTool = Join-Path ${env:ProgramFiles(x86)} "Windows Kits/10/bin/$SdkVersion/x64/makeappx.exe"
if (-not (Test-Path -LiteralPath $sdkTool)) { throw "Windows SDK MakeAppx missing: $sdkTool" }
$output = [System.IO.Path]::GetFullPath($OutputDirectory)
$payload = Join-Path $output 'identity-payload'
New-Item -ItemType Directory -Path $payload -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'AppxManifest.xml') -Destination $payload -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Assets') -Destination $payload -Recurse -Force
$package = Join-Path $output 'DaijobuIsland.Notifications.msix'
# Sparse package payload omits the external executable. Microsoft requires /nv
# for these external paths. This only creates an unsigned file; it never installs.
& $sdkTool pack /o /nv /d $payload /p $package
if ($LASTEXITCODE -ne 0) { throw "MakeAppx failed: $LASTEXITCODE" }
Write-Output "Unsigned identity package: $package"
Write-Output 'Sign and explicitly register per README.md; build does not grant identity or access.'
