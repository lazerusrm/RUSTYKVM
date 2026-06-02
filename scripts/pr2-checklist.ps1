param(
  [string]$BaseUrl = "http://192.168.0.84",
  [string]$User = "admin",
  [string]$Pass = "admin"
)

$ErrorActionPreference = "Stop"
$failures = @()

function Assert([bool]$cond, [string]$name) {
  if ($cond) { Write-Host "PASS $name" -ForegroundColor Green }
  else { Write-Host "FAIL $name" -ForegroundColor Red; $script:failures += $name }
}

function Encrypt-LoginPassword([string]$Password, [string]$SecretKey) {
  $escapedKey = $SecretKey.Replace("'", "\'")
  $escapedPass = $Password.Replace("'", "\'")
  node -e "const crypto=require('crypto');function evp(p,s,kl,il){let d=Buffer.alloc(0),b=Buffer.alloc(0);while(d.length<kl+il){const h=crypto.createHash('md5');if(b.length)h.update(b);h.update(p);h.update(s);b=h.digest();d=Buffer.concat([d,b]);}return{key:d.subarray(0,kl),iv:d.subarray(kl,kl+il)};}function enc(pw,k){const s=crypto.randomBytes(8);const {key,iv}=evp(Buffer.from(k,'utf8'),s,32,16);const c=crypto.createCipheriv('aes-256-cbc',key,iv);const e=Buffer.concat([c.update(pw,'utf8'),c.final()]);return Buffer.concat([Buffer.from('Salted__'),s,e]).toString('base64');}console.log(enc('$escapedPass','$escapedKey'));"
}

Write-Host "PR #2 checklist on $BaseUrl"
Write-Host "================================"

# 1. Unauthenticated password change
$code = (curl.exe -s -o NUL -w "%{http_code}" -X POST -H "Content-Type: application/json" -d "{}" "$BaseUrl/api/auth/password").Trim()
Assert ($code -eq "401") "unauthenticated POST /api/auth/password -> 401"

# 2. Encrypted login
$cookie = New-TemporaryFile
$keyResp = curl.exe -s "$BaseUrl/api/auth/encryption-key" | ConvertFrom-Json
Assert ($keyResp.code -eq 0) "encryption-key endpoint"
$enc = Encrypt-LoginPassword -Password $Pass -SecretKey $keyResp.data.key
$loginBody = (@{ username = $User; password = $enc } | ConvertTo-Json -Compress)
$loginResp = $loginBody | curl.exe -s -c $cookie.FullName -H "Content-Type: application/json" --data-binary "@-" "$BaseUrl/api/login" | ConvertFrom-Json
Assert ($loginResp.code -eq 0) "encrypted login"
$token = $loginResp.data.token

# Authenticated version
$code = (curl.exe -s -b $cookie.FullName -o NUL -w "%{http_code}" "$BaseUrl/api/application/version").Trim()
Assert ($code -eq "200") "authenticated /api/application/version"

# Leader-key
$lk = curl.exe -s -b $cookie.FullName "$BaseUrl/api/hid/leader-key" | ConvertFrom-Json
Assert ($lk.code -eq 0 -and $lk.data.key) "GET /api/hid/leader-key"

# MJPEG stream (first chunk)
$mj = curl.exe -s -b $cookie.FullName --max-time 3 "$BaseUrl/api/stream/mjpeg"
Assert ($mj -match "frame|jpeg|Content-Type") "authenticated MJPEG stream bytes"

# 3. Logout invalidates session cookie path
curl.exe -s -b $cookie.FullName -X POST "$BaseUrl/api/auth/logout" | Out-Null
$codeAfter = (curl.exe -s -b $cookie.FullName -o NUL -w "%{http_code}" "$BaseUrl/api/application/version").Trim()
Assert ($codeAfter -eq "401") "post-logout API -> 401"

# Re-login for brute-force test
$cookie2 = New-TemporaryFile
$loginBody | curl.exe -s -c $cookie2.FullName -H "Content-Type: application/json" --data-binary "@-" "$BaseUrl/api/login" | Out-Null

# 4. Brute-force lockout (wrong password x6)
$badBody = (@{ username = $User; password = (Encrypt-LoginPassword -Password "wrong-pass" -SecretKey $keyResp.data.key) } | ConvertTo-Json -Compress)
$locked = $false
for ($i = 1; $i -le 6; $i++) {
  $r = $badBody | curl.exe -s -H "Content-Type: application/json" --data-binary "@-" "$BaseUrl/api/login" | ConvertFrom-Json
  if ($r.code -eq -5 -or ($r.msg -match "locked")) { $locked = $true; break }
}
Assert $locked "brute-force lockout after repeated failures"

# 5. Capabilities require auth; passkey status remains public for login UI
$code = (curl.exe -s -o NUL -w "%{http_code}" "$BaseUrl/api/system/capabilities").Trim()
Assert ($code -eq "401") "unauthenticated /api/system/capabilities -> 401"
$code = (curl.exe -s -o NUL -w "%{http_code}" "$BaseUrl/api/passkey/status").Trim()
Assert ($code -eq "200") "public /api/passkey/status"

Remove-Item -Force $cookie.FullName, $cookie2.FullName -ErrorAction SilentlyContinue

Write-Host "================================"
if ($failures.Count -eq 0) {
  Write-Host "All checklist items passed."
  exit 0
}
Write-Host "Failed: $($failures -join ', ')"
exit 1