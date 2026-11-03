# dtk installer for Windows. Downloads the release, checks its SHA-256, installs dtk.exe.
#
#   irm https://raw.githubusercontent.com/amaye15/dtk/main/install.ps1 | iex
#
# Options (environment variables):
#   DTK_VERSION        a tag such as v0.4.0 (default: the latest release)
#   DTK_INSTALL_DIR    where to put dtk.exe (default: $env:USERPROFILE\.local\bin)
#   DTK_SKIP_CHECKSUM  set to 1 to skip the SHA-256 check (not recommended)
#   DTK_ADD_TO_PATH    set to 1 to add the install folder to your user PATH
$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$ProgressPreference = 'SilentlyContinue'

function Info($m) { Write-Host "dtk install: $m" }
function Fail($m) { Write-Error "dtk install: $m"; exit 1 }

# GitHub serves release files as application/octet-stream, and then Invoke-WebRequest and
# Invoke-RestMethod return the body as bytes, not text. Save the file and read it as text.
function Get-Text($url) {
  $f = Join-Path ([IO.Path]::GetTempPath()) ("dtk-" + [Guid]::NewGuid().ToString('N') + '.txt')
  try {
    Invoke-WebRequest $url -OutFile $f
    return [IO.File]::ReadAllText($f)
  } finally { Remove-Item $f -Force -ErrorAction SilentlyContinue }
}

$repo = 'amaye15/dtk'
$dest = if ($env:DTK_INSTALL_DIR) { $env:DTK_INSTALL_DIR } else { Join-Path $env:USERPROFILE '.local\bin' }
if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64' -and $env:PROCESSOR_ARCHITEW6432 -ne 'AMD64') {
  Info "only the x86_64 build exists; on this CPU it runs through Windows' emulation"
}

$version = $env:DTK_VERSION
if (-not $version) {
  try { $version = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name }
  catch { Fail "could not find the latest release (GitHub may be rate-limiting; set DTK_VERSION=vX.Y.Z to pin)" }
}

$name = "dtk-$version-x86_64-pc-windows-msvc"
$archive = "$name.zip"
$base = "https://github.com/$repo/releases/download/$version"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("dtk-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
  Info "downloading $archive"
  try { Invoke-WebRequest "$base/$archive" -OutFile (Join-Path $tmp $archive) }
  catch { Fail "no Windows release file at $version" }

  if ($env:DTK_SKIP_CHECKSUM -eq '1') {
    Info 'DTK_SKIP_CHECKSUM=1 is set: SKIPPING the checksum check (not recommended)'
  } else {
    $want = $null
    try {
      $sums = (Get-Text "$base/checksums.txt") -split "`n"
      $line = $sums | Where-Object { $_ -match "\s$([regex]::Escape($archive))\s*$" } | Select-Object -First 1
      if ($line) { $want = ($line -split '\s+')[0] }
    } catch { }
    if (-not $want) {
      try { $want = ((Get-Text "$base/$archive.sha256").Trim() -split '\s+')[0] } catch { }
    }
    if (-not $want) { Fail "could not download a checksum for $archive; refusing to install an unverified file (DTK_SKIP_CHECKSUM=1 bypasses this)" }
    $got = (Get-FileHash (Join-Path $tmp $archive) -Algorithm SHA256).Hash.ToLower()
    if ($want.ToLower() -ne $got) { Fail "checksum mismatch (expected $want, got $got); not installing" }
    Info 'checksum ok'
  }

  Expand-Archive (Join-Path $tmp $archive) -DestinationPath (Join-Path $tmp 'x') -Force
  $exe = Get-ChildItem (Join-Path $tmp 'x') -Recurse -Filter 'dtk.exe' | Select-Object -First 1
  if (-not $exe) { Fail 'the archive does not contain dtk.exe' }
  New-Item -ItemType Directory -Force $dest | Out-Null
  Copy-Item $exe.FullName (Join-Path $dest 'dtk.exe') -Force
  Info ("installed " + (& (Join-Path $dest 'dtk.exe') --version) + " to " + (Join-Path $dest 'dtk.exe'))

  $onPath = ($env:PATH -split ';') -contains $dest
  if (-not $onPath) {
    if ($env:DTK_ADD_TO_PATH -eq '1') {
      $user = [Environment]::GetEnvironmentVariable('PATH', 'User')
      [Environment]::SetEnvironmentVariable('PATH', "$user;$dest", 'User')
      Info "added $dest to your user PATH (open a new terminal)"
    } else {
      Info "$dest is not on your PATH. Add it with: [Environment]::SetEnvironmentVariable('PATH', `"`$([Environment]::GetEnvironmentVariable('PATH','User'));$dest`", 'User')   (or rerun with DTK_ADD_TO_PATH=1)"
    }
  }
  # Register the hook, so that nothing is left to do. Skip with DTK_NO_INIT=1. Claude Code must
  # be there (its folder exists): dtk init writes into settings.json there and keeps a backup.
  $dtkExe = Join-Path $dest 'dtk.exe'
  $claude = Join-Path $env:USERPROFILE '.claude'
  if ($env:DTK_NO_INIT -eq '1') {
    Info 'DTK_NO_INIT=1 is set: the hook is not registered. When you want it: dtk init --global'
  } elseif (Test-Path $claude) {
    Info 'registering the hook in ~\.claude\settings.json (a backup is kept; undo with: dtk init --uninstall)'
    & $dtkExe init --global
    if ($LASTEXITCODE -eq 0) {
      Info 'done. Start a new Claude Code session; check the setup with: dtk verify'
    } else {
      Info "could not register the hook. Run it yourself and read its message: $dtkExe init --global"
    }
  } else {
    Info "Claude Code's folder (~\.claude) was not found. After you install Claude Code, run: dtk init --global"
  }
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
