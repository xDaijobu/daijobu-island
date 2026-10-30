param(
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [string]$SdkVersion = '10.0.26100.0',
    [ValidatePattern('^[0-9a-fA-F]{40}$')][string]$CertificateThumbprint,
    [string]$TimestampUrl
)
$ErrorActionPreference = 'Stop'
$sdkTool = Join-Path ${env:ProgramFiles(x86)} "Windows Kits/10/bin/$SdkVersion/x64/makeappx.exe"
if (-not (Test-Path -LiteralPath $sdkTool)) { throw "Windows SDK MakeAppx missing: $sdkTool" }
$output = [System.IO.Path]::GetFullPath($OutputDirectory)
$payload = Join-Path $output ('identity-payload-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $payload -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'AppxManifest.xml') -Destination $payload -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Assets') -Destination $payload -Recurse -Force
$package = Join-Path $output 'DaijobuIsland.Notifications.msix'
# Sparse package payload omits the external executable. Microsoft requires /nv
# for these external paths. This only creates an unsigned file; it never installs.
& $sdkTool pack /o /nv /d $payload /p $package
if ($LASTEXITCODE -ne 0) { throw "MakeAppx failed: $LASTEXITCODE" }
Write-Output "Unsigned identity package: $package"
if ($CertificateThumbprint) {
    $signTool = Join-Path ${env:ProgramFiles(x86)} "Windows Kits/10/bin/$SdkVersion/x64/signtool.exe"
    if (-not (Test-Path -LiteralPath $signTool)) { throw "Windows SDK SignTool missing: $signTool" }
    $cert = Get-Item -LiteralPath "Cert:/CurrentUser/My/$CertificateThumbprint"
    [xml]$manifest = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'AppxManifest.xml')
    if ($cert.Subject -cne $manifest.Package.Identity.Publisher) { throw 'Signing certificate Subject must exactly match the package Publisher and app/daijobu.manifest publisher.' }
    if (-not $cert.HasPrivateKey) { throw 'Signing certificate must have an accessible private key on this build machine.' }
    $signArgs = @('sign', '/fd', 'SHA256', '/sha1', $CertificateThumbprint, '/s', 'My')
    if ($TimestampUrl) { $signArgs += @('/tr', $TimestampUrl, '/td', 'SHA256') }
    & $signTool @signArgs $package
    if ($LASTEXITCODE -ne 0) { throw "SignTool sign failed: $LASTEXITCODE" }
    & $signTool verify /pa /all $package
    if ($LASTEXITCODE -ne 0) { throw "SignTool verification failed: $LASTEXITCODE" }
    Write-Output "Verified signed identity payload: $package"
    Write-Output 'Set DAIJOBU_NOTIFICATION_MSIX to this absolute path before building the distribution exe.'
} else {
    Write-Output 'Unsigned build artifact only: do not embed or distribute this MSIX. Supply -CertificateThumbprint to sign with an existing build-machine certificate.'
}
Write-Output 'This script never registers packages, creates certificates, or changes trust.'
