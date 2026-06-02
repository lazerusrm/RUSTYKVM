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

  $VersionLine = (Get-Content (Join-Path $RepoRoot "server/Cargo.toml") | Select-String -Pattern '^version\s*=' | Select-Object -First 1).Line
  $Version = (($VersionLine -split '\"')[1]).Trim()

  Write-Host "Installing RUSTYKVM init script + restarting service..."
  $InitScript = Join-Path $RepoRoot "scripts/S95nanokvm"
  if (Test-Path $InitScript) {
    scp $InitScript "${SshHost}:/etc/init.d/S95nanokvm.new" | Out-Null
  }

  $remoteCmd =
    'set -e; ' +
    'for pid in $(pidof nanokvm-server 2>/dev/null); do kill "$pid" 2>/dev/null || true; done; ' +
    'sleep 1; ' +
    'if [ -f /etc/init.d/S95nanokvm.new ]; then mv -f /etc/init.d/S95nanokvm.new /etc/init.d/S95nanokvm; chmod +x /etc/init.d/S95nanokvm; fi; ' +
    'TS=$(date +%s); ' +
    'if [ -f ' + $TargetPath + ' ]; then cp -f ' + $TargetPath + ' ' + $TargetPath + '.bak.$TS; fi; ' +
    'mv -f ' + $RemoteNew + ' ' + $TargetPath + '; ' +
    'chmod +x ' + $TargetPath + '; ' +
    'echo v' + $Version + ' > /kvmapp/version; ' +
    'if [ -x /etc/init.d/S95nanokvm ]; then /etc/init.d/S95nanokvm restart; ' +
    'else ' + $TargetPath + ' >> /var/log/nanokvm.log 2>&1 & fi'
  ssh $SshHost $remoteCmd | Out-Null
  if ($LASTEXITCODE -ne 0) {
    throw "remote activate/restart failed with exit code $LASTEXITCODE"
  }

  Write-Host "Waiting for HTTP health..."
  $healthy = $false
  for ($i = 0; $i -lt 30; $i++) {
    $code = (ssh $SshHost "curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1/health 2>/dev/null").Trim()
    if ($code -eq "200") {
      $healthy = $true
      break
    }
    Start-Sleep -Seconds 1
  }
  if (-not $healthy) {
    throw "service did not return HTTP 200 on /health after restart (device may use http only, not https)"
  }

  Write-Host "Done."
}
finally {
  Pop-Location
}
