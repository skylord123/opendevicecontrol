# Download the PawnIO driver installer and its signed modules for the
# Windows installer (packaging\windows\opendevicecontrol.iss bundles them), into
# packaging\windows\pawnio\ (not committed). Versions and SHA-256 are pinned.
#
#   powershell -ExecutionPolicy Bypass -File packaging\windows\fetch-pawnio.ps1
#
# PawnIO (https://pawnio.eu, namazso): the signed kernel driver OpenDeviceControl's
# sensor service and RAM lighting use. Its installer and modules are
# redistributable (GPL-2.0 / LGPL-2.1; see pawnio\COPYING).

$ErrorActionPreference = 'Stop'
$dir = Join-Path $PSScriptRoot 'pawnio'
$files = @(
    @{
        Name = 'PawnIO_setup.exe'
        Url = 'https://github.com/namazso/PawnIO.Setup/releases/download/2.2.0/PawnIO_setup.exe'
        Sha256 = '1f519a22e47187f70a1379a48ca604981c4fcf694f4e65b734aaa74a9fba3032'
    },
    @{
        Name = 'modules.zip'
        Url = 'https://github.com/namazso/PawnIO.Modules/releases/download/0.2.11/release_0_2_11.zip'
        Sha256 = '43608cb89bc84247fef1368a139013f7d043e17db6d6c8dfc9b46bf0905a81f4'
    }
)

New-Item -ItemType Directory -Force (Join-Path $dir 'modules') | Out-Null
foreach ($f in $files) {
    $path = Join-Path $dir $f.Name
    if (-not (Test-Path $path) -or (Get-FileHash $path -Algorithm SHA256).Hash -ne $f.Sha256) {
        Write-Host "Downloading $($f.Url)"
        Invoke-WebRequest $f.Url -OutFile $path -UseBasicParsing
    }
    $hash = (Get-FileHash $path -Algorithm SHA256).Hash
    if ($hash -ne $f.Sha256) {
        Remove-Item $path
        throw "$($f.Name): SHA-256 $hash, expected $($f.Sha256)"
    }
}
Expand-Archive (Join-Path $dir 'modules.zip') -DestinationPath (Join-Path $dir 'modules') -Force
$n = (Get-ChildItem (Join-Path $dir 'modules') -Filter *.bin).Count
if ($n -eq 0) { throw 'no modules in the PawnIO.Modules archive' }
Write-Host "PawnIO installer and $n modules in $dir"
