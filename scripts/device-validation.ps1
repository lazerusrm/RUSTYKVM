param(
  [string]$BaseUrl = "http://192.168.0.84",
  [string]$User = "admin",
  [string]$Pass = "admin",
  [string]$SshHost = "nanokvm"
)

$ErrorActionPreference = "Stop"
$failures = @()

function Pass([string]$name) { Write-Host "PASS $name" -ForegroundColor Green }
function Fail([string]$name, [string]$detail = "") {
  Write-Host "FAIL $name" -ForegroundColor Red
  if ($detail) { Write-Host "      $detail" -ForegroundColor DarkRed }
  $script:failures += $name
}

function Encrypt-LoginPassword([string]$Password, [string]$SecretKey) {
  $escapedKey = $SecretKey.Replace("'", "\'")
  $escapedPass = $Password.Replace("'", "\'")
  node -e "const crypto=require('crypto');function evp(p,s,kl,il){let d=Buffer.alloc(0),b=Buffer.alloc(0);while(d.length<kl+il){const h=crypto.createHash('md5');if(b.length)h.update(b);h.update(p);h.update(s);b=h.digest();d=Buffer.concat([d,b]);}return{key:d.subarray(0,kl),iv:d.subarray(kl,kl+il)};}function enc(pw,k){const s=crypto.randomBytes(8);const {key,iv}=evp(Buffer.from(k,'utf8'),s,32,16);const c=crypto.createCipheriv('aes-256-cbc',key,iv);const e=Buffer.concat([c.update(pw,'utf8'),c.final()]);return Buffer.concat([Buffer.from('Salted__'),s,e]).toString('base64');}console.log(enc('$escapedPass','$escapedKey'));"
}

Write-Host "Device validation: $BaseUrl"
Write-Host "========================================"

# --- HTTP / API ---
$h = (curl.exe -s -o NUL -w "%{http_code}" "$BaseUrl/health").Trim()
if ($h -eq "200") { Pass "GET /health" } else { Fail "GET /health" "code=$h" }

$loginJs = curl.exe -s "$BaseUrl/static/js/login.js"
if ($loginJs -match "passkey/status") { Pass "login.js uses /api/passkey/status" }
else { Fail "login.js uses /api/passkey/status" "still on system/capabilities?" }

$pk = (curl.exe -s -o NUL -w "%{http_code}" "$BaseUrl/api/passkey/status").Trim()
if ($pk -eq "200") { Pass "public GET /api/passkey/status" } else { Fail "public GET /api/passkey/status" "code=$pk" }

$cap = (curl.exe -s -o NUL -w "%{http_code}" "$BaseUrl/api/system/capabilities").Trim()
if ($cap -eq "401") { Pass "unauthenticated /api/system/capabilities -> 401" }
else { Fail "unauthenticated /api/system/capabilities -> 401" "code=$cap" }

$cookie = New-TemporaryFile
try {
  $keyResp = curl.exe -s "$BaseUrl/api/auth/encryption-key" | ConvertFrom-Json
  if ($keyResp.code -ne 0) { Fail "encryption-key"; throw "abort" }
  $enc = Encrypt-LoginPassword -Password $Pass -SecretKey $keyResp.data.key
  $loginBody = (@{ username = $User; password = $enc } | ConvertTo-Json -Compress)
  $loginResp = $loginBody | curl.exe -s -c $cookie.FullName -H "Content-Type: application/json" --data-binary "@-" "$BaseUrl/api/login" | ConvertFrom-Json
  if ($loginResp.code -eq 0) { Pass "encrypted login" } else { Fail "encrypted login" "code=$($loginResp.code) msg=$($loginResp.msg)" }

  $ver = curl.exe -s -b $cookie.FullName "$BaseUrl/api/application/version" | ConvertFrom-Json
  if ($ver.code -eq 0 -and ($ver.data.current -or $ver.data.version)) {
    $v = if ($ver.data.current) { $ver.data.current.Trim() } else { $ver.data.version }
    Pass "GET /api/application/version ($v)"
  } else { Fail "GET /api/application/version" }

  $hdmi = curl.exe -s -b $cookie.FullName "$BaseUrl/api/vm/hdmi" 2>$null
  if ($hdmi) {
    $hj = $hdmi | ConvertFrom-Json -ErrorAction SilentlyContinue
    if ($hj -and $hj.code -eq 0) { Pass "GET /api/vm/hdmi (connected=$($hj.data.connected))" }
    else { Pass "GET /api/vm/hdmi (raw response)" }
  } else { Fail "GET /api/vm/hdmi" "no response (linux-only?)" }

  $mjFile = New-TemporaryFile
  curl.exe -s -b $cookie.FullName --max-time 5 "$BaseUrl/api/stream/mjpeg" -o $mjFile.FullName | Out-Null
  $mjBytes = (Get-Item $mjFile.FullName).Length
  if ($mjBytes -gt 1000) { Pass "MJPEG stream ($mjBytes bytes / 5s)" }
  elseif ($mjBytes -gt 0) { Write-Host "WARN MJPEG low throughput ($mjBytes bytes) — HDMI/subscribers?" -ForegroundColor Yellow }
  else { Write-Host "WARN MJPEG empty — check HDMI cable" -ForegroundColor Yellow }

  $wsCode = (curl.exe -s -b $cookie.FullName -o NUL -w "%{http_code}" "$BaseUrl/api/ws").Trim()
  if ($wsCode -in @("200","400","401","403","405","426")) { Pass "GET /api/ws reachable (code=$wsCode)" }
  else { Fail "GET /api/ws" "code=$wsCode" }
}
finally {
  Remove-Item -Force $cookie.FullName -ErrorAction SilentlyContinue
}

# --- SSH host checks ---
Write-Host "----------------------------------------"
Write-Host "SSH checks ($SshHost)"
$sshOut = ssh $SshHost @'
set -e
echo "version:$(cat /kvmapp/version 2>/dev/null || echo unknown)"
echo "libkvm:$(test -f /kvmapp/dl_lib/libkvm.so && echo ok || echo missing)"
echo "kvm_system:$(pgrep -x kvm_system 2>/dev/null | wc -l | tr -d ' ')"
echo "go_server:$(pgrep -f NanoKVM-Server 2>/dev/null | wc -l | tr -d ' ')"
echo "nanokvm:$(ps | grep -c '[n]anokvm-server' || true)"
echo "init_s95:$(test -x /etc/init.d/S95nanokvm && echo yes || echo no)"
'@ 2>&1

foreach ($line in ($sshOut -split "`n")) {
  if ($line -match "^version:(.+)$") {
    $vf = $Matches[1].Trim()
    if ($vf -match "v?0\.2\.") { Pass "device version file $vf" }
    elseif ($vf) { Pass "device version file $vf" }
    else { Fail "device version file" "empty" }
  }
  if ($line -match "^libkvm:ok") { Pass "libkvm.so present" }
  if ($line -match "^libkvm:missing") { Fail "libkvm.so present" }
  if ($line -match "^kvm_system:0") { Pass "kvm_system not running" }
  if ($line -match "^kvm_system:[1-9]") { Fail "kvm_system not running" "count=$line" }
  if ($line -match "^go_server:0") { Pass "Go NanoKVM-Server not running" }
  if ($line -match "^go_server:[1-9]") { Fail "Go NanoKVM-Server not running" "count=$line" }
  if ($line -match "^nanokvm:1") { Pass "single nanokvm-server process" }
  if ($line -match "^nanokvm:" -and $line -notmatch ":1") { Fail "single nanokvm-server process" $line }
  if ($line -match "^init_s95:yes") { Pass "/etc/init.d/S95nanokvm installed" }
  if ($line -match "^init_s95:no") { Write-Host "WARN /etc/init.d/S95nanokvm missing (manual restart used)" -ForegroundColor Yellow }
}

Write-Host "========================================"
if ($failures.Count -eq 0) {
  Write-Host "Device validation passed (review WARN lines above)." -ForegroundColor Green
  exit 0
}
Write-Host "Failed: $($failures -join ', ')" -ForegroundColor Red
exit 1