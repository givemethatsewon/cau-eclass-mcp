# Install a prebuilt eclass-mcp Rust binary on Windows.
$ErrorActionPreference = "Stop"
$Repo = if ($env:ECLASS_REPO) { $env:ECLASS_REPO } else { "givemethatsewon/cau-eclass-mcp" }
$Version = if ($env:ECLASS_VERSION) { $env:ECLASS_VERSION } else { "latest" }
$InstallDir = if ($env:ECLASS_INSTALL_DIR) { $env:ECLASS_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "eclass-mcp\bin" }

$Arch = if ([System.Environment]::Is64BitOperatingSystem) {
    if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
} else {
    $env:PROCESSOR_ARCHITECTURE
}
if ($Arch -ne "AMD64") {
    throw "Unsupported Windows architecture: $Arch. This installer supports Windows x64."
}
$Target = "x86_64-pc-windows-msvc"
$Asset = "eclass-mcp-$Target.zip"
$Base = if ($Version -eq "latest") {
    "https://github.com/$Repo/releases/latest/download"
} else {
    "https://github.com/$Repo/releases/download/$Version"
}

$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("eclass-mcp-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $Tmp | Out-Null
try {
    Write-Host "Downloading $Asset..."
    Invoke-WebRequest -UseBasicParsing "$Base/$Asset" -OutFile (Join-Path $Tmp $Asset)
    Invoke-WebRequest -UseBasicParsing "$Base/$Asset.sha256" -OutFile (Join-Path $Tmp "$Asset.sha256")

    $Expected = (Get-Content (Join-Path $Tmp "$Asset.sha256")).Split(" ")[0].ToLower()
    $Actual = (Get-FileHash (Join-Path $Tmp $Asset) -Algorithm SHA256).Hash.ToLower()
    if ($Expected -ne $Actual) { throw "SHA-256 mismatch" }

    Expand-Archive -Path (Join-Path $Tmp $Asset) -DestinationPath $Tmp
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $Executable = Join-Path $InstallDir "eclass-mcp.exe"
    Copy-Item (Join-Path $Tmp "eclass-mcp-$Target\eclass-mcp.exe") $Executable -Force

    & $Executable --version
    if ($LASTEXITCODE -ne 0) { throw "eclass-mcp --version failed (exit $LASTEXITCODE)" }
    Write-Host "Installed to $Executable"
    $QuotedExecutable = $Executable.Replace("'", "''")
    Write-Host "Next: & '$QuotedExecutable' onboard --client claude"
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
