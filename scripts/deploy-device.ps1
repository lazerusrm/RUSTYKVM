param(
  [string]$SshHost = "nanokvm",
  [string]$TargetPath = "/kvmapp/nanokvm-server"
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $RepoRoot
try {
  Write-Host "Building (riscv64gc-unknown-linux-musl) via docker..."
  docker run --rm -v ${PWD}:/home/rust/src messense/rust-musl-cross:riscv64gc-musl `
    cargo build -p nanokvm-server --release

  $LocalBin = Join-Path $RepoRoot "target/riscv64gc-unknown-linux-musl/release/nanokvm-server"
  if (!(Test-Path $LocalBin)) {
    throw "missing build output: $LocalBin"
  }

  $LocalSha = (Get-FileHash $LocalBin -Algorithm SHA256).Hash.ToLowerInvariant()
  $RemoteNew = "${TargetPath}.new"

  Write-Host "Uploading binary to ${SshHost}:$RemoteNew"
  scp $LocalBin "${SshHost}:$RemoteNew" | Out-Null
  if ($LASTEXITCODE -ne 0) {
    throw "scp failed with exit code $LASTEXITCODE"
  }

  Write-Host "Verifying SHA256 on device..."
  $RemoteSha = (ssh $SshHost "sha256sum $RemoteNew | cut -d ' ' -f 1").Trim().ToLowerInvariant()
  if ($LASTEXITCODE -ne 0) {
    throw "sha256sum failed with exit code $LASTEXITCODE"
  }
  if ($RemoteSha -ne $LocalSha) {
    throw "sha256 mismatch: local=$LocalSha remote=$RemoteSha"
  }

  $VersionLine = (Get-Content (Join-Path $RepoRoot "server/Cargo.toml") | Select-String -Pattern '^version\\s*=' | Select-Object -First 1).Line
  $Version = (($VersionLine -split '\"')[1]).Trim()

  Write-Host "Activating + restarting service..."
  $remoteCmd =
    'set -e; ' +
    'TS=$(date +%s); ' +
    'if [ -f ' + $TargetPath + ' ]; then cp -f ' + $TargetPath + ' ' + $TargetPath + '.bak.$TS; fi; ' +
    'mv -f ' + $RemoteNew + ' ' + $TargetPath + '; ' +
    'chmod +x ' + $TargetPath + '; ' +
    'echo v' + $Version + ' > /kvmapp/version; ' +
    '/etc/init.d/S95nanokvm restart'
  ssh $SshHost $remoteCmd | Out-Null
  if ($LASTEXITCODE -ne 0) {
    throw "remote activate/restart failed with exit code $LASTEXITCODE"
  }

  Write-Host "Done."
}
finally {
  Pop-Location
}
