<#
.SYNOPSIS
Download and build ONNX Runtime v1.22.0 into target/onnxruntime.

.DESCRIPTION
Matches scripts/build_onnxruntime.sh logic, including patch application and optional operator trimming.

.PARAMETER OpsConfig
Path to the operator config file to be passed to --include_ops_by_config.

.PARAMETER Help
Show help message.

.EXAMPLE
.\build_onnxruntime.ps1 -OpsConfig handpose_estimation_mediapipe/required_operators.config

.NOTES
Environment overrides:
  OUT_DIR     Where to place downloads and sources (default: <repo>/target/onnxruntime)
  ORT_VERSION ONNX Runtime tag to fetch (default: 1.22.0)
  OPS_CONFIG  Operator config path passed to --include_ops_by_config
#>

param(
    [string]$OpsConfig,
    [switch]$Help
)

function Show-Usage {
    @"
Usage: scripts/build_onnxruntime.ps1 [-OpsConfig path] [-Help]

Environment overrides:
  OUT_DIR     Where to place downloads and sources (default: <repo>\target\onnxruntime)
  ORT_VERSION ONNX Runtime tag to fetch (default: 1.22.0)
  OPS_CONFIG  Operator config path passed to --include_ops_by_config
"@ | Write-Host
    exit 0
}

if ($Help) {
    Show-Usage
}

if (-not $OpsConfig -and $env:OPS_CONFIG) {
    $OpsConfig = $env:OPS_CONFIG
}

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$OutDir = if ($env:OUT_DIR) { $env:OUT_DIR } else { Join-Path $root "target\onnxruntime" }
$OrtVersion = if ($env:ORT_VERSION) { $env:ORT_VERSION } else { "1.22.0" }

$SrcDir = Join-Path $OutDir "onnxruntime-$OrtVersion"
$Archive = Join-Path $OutDir "onnxruntime-$OrtVersion.tar.gz"

# Convert to absolute path if relative
if ($OpsConfig) {
    if (-not [System.IO.Path]::IsPathRooted($OpsConfig)) {
        $OpsConfig = Join-Path $root $OpsConfig
    }
    if (-not (Test-Path $OpsConfig)) {
        Write-Error "Specified ops config not found: $OpsConfig"
        exit 1
    }
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

if (-not (Test-Path $SrcDir)) {
    Write-Host "Downloading ONNX Runtime v$OrtVersion sources..."
    Invoke-WebRequest "https://github.com/microsoft/onnxruntime/archive/refs/tags/v$OrtVersion.tar.gz" -OutFile $Archive
    & tar -xzf $Archive -C $OutDir
}

# Fix Eigen SHA1 hash mismatch in deps.txt
$DepsFile = Join-Path $SrcDir "cmake\deps.txt"
if (Test-Path $DepsFile) {
    Write-Host "Fixing Eigen SHA1 hash in deps.txt..."
    Copy-Item $DepsFile "$DepsFile.bak" -Force
    (Get-Content -Raw $DepsFile) -replace '5ea4d05e62d7f954a46b3213f9b2535bdd866803', '51982be81bbe52572b54180454df11a3ece9a934' | Set-Content $DepsFile
}

# Download and apply patches from ort-artifacts
$PatchesRepo = Join-Path $OutDir "ort-artifacts"
$PatchesDir = Join-Path $PatchesRepo "src/patches/all"
$PatchesCommit = "77ec493e3495901a361469951ab992181e52fd05"

if (-not (Test-Path $PatchesRepo)) {
    Write-Host "Cloning ort-artifacts repository..."
    git clone https://github.com/pykeio/ort-artifacts.git $PatchesRepo
    Push-Location $PatchesRepo
    git checkout $PatchesCommit | Out-Null
    Pop-Location
} else {
    Write-Host "ort-artifacts repository already exists, using existing patches..."
}

if (-not (Test-Path $PatchesDir)) {
    Write-Error "Patches directory not found: $PatchesDir"
    exit 1
}

Write-Host "Applying patches to ONNX Runtime sources..."

$PatchExe = "patch"
if (-not (Get-Command patch -ErrorAction SilentlyContinue)) {
    $GitPatch = "C:\Program Files\Git\usr\bin\patch.exe"
    if (Test-Path $GitPatch) {
        $PatchExe = $GitPatch
    }
}

Push-Location $SrcDir
Get-ChildItem -Path $PatchesDir -Filter "*.patch" | ForEach-Object {
    $patch = $_
    Write-Host "  Applying $($patch.Name)..."
    & $PatchExe -p1 -N -r - -i $patch.FullName 2>$null
    if ($LASTEXITCODE -eq 0) {
        Write-Host "    Applied successfully"
    } else {
        Write-Host "    Skipped (already applied or not applicable)"
    }
}
Pop-Location

Push-Location $SrcDir

# Set build directory to platform-independent location
$LibDir = Join-Path $OutDir "build"

$buildArgs = @(
    "--config", "MinSizeRel",
    "--parallel",
    "--skip_tests",
    "--disable_ml_ops",
    "--disable_rtti",
    "--build_dir", $LibDir
)

if ($OpsConfig) {
    Write-Host "Using operator config at $OpsConfig"
    $buildArgs += @("--include_ops_by_config", $OpsConfig)
}

& ".\build.bat" @buildArgs
if ($LASTEXITCODE -ne 0) {
    Pop-Location
    throw "Build failed with exit code $LASTEXITCODE"
}

Pop-Location

Write-Host "ONNX Runtime build finished under $SrcDir"
Write-Host "Build artifacts available at $LibDir"
