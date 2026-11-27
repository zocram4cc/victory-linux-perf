# Puts the DirectX 12 Agility SDK runtime (D3D12Core.dll) in <Dest>\D3D12\,
# next to victory.exe. Windows 10's own D3D12 has no shader model 6.6, mesh
# shaders or directly indexed heaps; with this folder beside victory.exe it
# gets them (apps/victory/src/agility.rs). Ship the D3D12 folder in every
# release zip.
#
#   powershell -ExecutionPolicy Bypass -File scripts\get-agility-sdk.ps1 -Dest <folder with victory.exe>
#
# -DebugLayer also installs d3d12SDKLayers.dll (VICTORY_D3D12_DEBUG in a
# diagnostics build). The version must match D3D12SDKVersion in
# crates/victory-gpu/src/requirements.rs (AGILITY_SDK_VERSION).
param(
    [Parameter(Mandatory = $true)][string]$Dest,
    [switch]$DebugLayer
)
$ErrorActionPreference = 'Stop'

$version = '1.620.2'
$url = "https://api.nuget.org/v3-flatcontainer/microsoft.direct3d.d3d12/$version/microsoft.direct3d.d3d12.$version.nupkg"
# SHA-256 of build/native/bin/x64/D3D12Core.dll in that package.
$coreSha256 = '70A6D47D335B4A15180544C01A35B84474522A369DC582D85FE057313A3AEAA0'

$out = Join-Path $Dest 'D3D12'
$core = Join-Path $out 'D3D12Core.dll'
$want = @('D3D12Core.dll')
if ($DebugLayer) { $want += 'd3d12SDKLayers.dll' }

$have = (Test-Path $core) -and ((Get-FileHash $core -Algorithm SHA256).Hash -eq $coreSha256)
if ($have -and (-not $DebugLayer -or (Test-Path (Join-Path $out 'd3d12SDKLayers.dll')))) {
    Write-Host "Agility SDK $version already in $out"
    exit 0
}

$work = Join-Path ([IO.Path]::GetTempPath()) ("agility-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $work | Out-Null
try {
    $pkg = Join-Path $work 'd3d12.nupkg'
    Write-Host "Downloading Microsoft.Direct3D.D3D12 $version from nuget.org"
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $pkg
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($pkg)
    try {
        New-Item -ItemType Directory -Force $out | Out-Null
        foreach ($name in $want) {
            $entry = $zip.Entries | Where-Object { $_.FullName -eq "build/native/bin/x64/$name" }
            if (-not $entry) { throw "the package has no build/native/bin/x64/$name" }
            $tmp = Join-Path $work $name
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $tmp, $true)
            $sig = Get-AuthenticodeSignature $tmp
            if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
                throw "$name is not signed by Microsoft ($($sig.Status))"
            }
            if ($name -eq 'D3D12Core.dll' -and (Get-FileHash $tmp -Algorithm SHA256).Hash -ne $coreSha256) {
                throw "D3D12Core.dll is not the expected $version build"
            }
            Copy-Item -Force $tmp (Join-Path $out $name)
        }
    } finally { $zip.Dispose() }
    Write-Host "Agility SDK $version installed in $out"
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
