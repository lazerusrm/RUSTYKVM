$ErrorActionPreference = "Stop"

param(
  [string]$Host = "nanokvm",
  [string]$TargetPath = "/kvmapp/nanokvm-server"
)

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

  Write-Host "Uploading binary to $Host:$RemoteNew"
  scp $LocalBin "${Host}:$RemoteNew" | Out-Null

  Write-Host "Verifying SHA256 on device..."
  $RemoteSha = (ssh $Host "sha256sum $RemoteNew | cut -d ' ' -f 1").Trim().ToLowerInvariant()
  if ($RemoteSha -ne $LocalSha) {
    throw "sha256 mismatch: local=$LocalSha remote=$RemoteSha"
  }

  Write-Host "Activating + restarting service..."
  ssh $Host @"
set -e
TS=\$(date +%s)
if [ -f "$TargetPath" ]; then
  cp -f "$TargetPath" "$TargetPath.bak.\$TS"
fi
mv -f "$RemoteNew" "$TargetPath"
chmod +x "$TargetPath"
/etc/init.d/S95nanokvm restart
"@ | Out-Null

  Write-Host "Done."
}
finally {
  Pop-Location
}

