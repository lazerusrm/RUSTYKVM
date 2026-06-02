param(
  [string]$BaseUrl = "http://192.168.0.84",
  [switch]$RunE2E,
  [string]$User = $env:NANOKVM_USER,
  [string]$Pass = $env:NANOKVM_PASS
)

$ErrorActionPreference = "Stop"

function Assert-Http200([string]$Url) {
  $last = ""
  for ($i = 0; $i -lt 30; $i++) {
    $last = (curl.exe -k -s -o NUL -w "%{http_code}" $Url).Trim()
    if ($last -eq "200") {
      return
    }
    Start-Sleep -Seconds 1
  }
  throw "expected 200 from $Url, got $last"
}

function Encrypt-LoginPassword([string]$Password, [string]$SecretKey) {
  $escapedKey = $SecretKey.Replace("'", "\'")
  $escapedPass = $Password.Replace("'", "\'")
  $encrypted = node -e "const crypto=require('crypto');function evp(p,s,kl,il){let d=Buffer.alloc(0),b=Buffer.alloc(0);while(d.length<kl+il){const h=crypto.createHash('md5');if(b.length)h.update(b);h.update(p);h.update(s);b=h.digest();d=Buffer.concat([d,b]);}return{key:d.subarray(0,kl),iv:d.subarray(kl,kl+il)};}function enc(pw,k){const s=crypto.randomBytes(8);const {key,iv}=evp(Buffer.from(k,'utf8'),s,32,16);const c=crypto.createCipheriv('aes-256-cbc',key,iv);const e=Buffer.concat([c.update(pw,'utf8'),c.final()]);return Buffer.concat([Buffer.from('Salted__'),s,e]).toString('base64');}console.log(enc('$escapedPass','$escapedKey'));"
  if (-not $encrypted) {
    throw "node is required to encrypt the login password (install Node.js)"
  }
  return $encrypted.Trim()
}

Assert-Http200 "$BaseUrl/health"
Assert-Http200 "$BaseUrl/login.html"
Assert-Http200 "$BaseUrl/api/system/capabilities"

if ($User -and $Pass) {
  $cookie = New-TemporaryFile
  try {
    $keyResp = curl.exe -k -s "$BaseUrl/api/auth/encryption-key"
    $keyJson = $keyResp | ConvertFrom-Json
    if ($keyJson.code -ne 0 -or -not $keyJson.data.key) {
      throw "failed to fetch encryption key from $BaseUrl/api/auth/encryption-key: $keyResp"
    }
    $encryptedPass = Encrypt-LoginPassword -Password $Pass -SecretKey $keyJson.data.key
    $loginBody = (@{ username = $User; password = $encryptedPass } | ConvertTo-Json -Compress)

    $ok = $false
    $lastResp = ""
    for ($i = 0; $i -lt 30; $i++) {
      try {
        $lastResp = $loginBody | curl.exe -k -s -c $cookie.FullName -H "Content-Type: application/json" --data-binary "@-" "$BaseUrl/api/login"
        $json = $lastResp | ConvertFrom-Json
        if ($json.code -eq 0) {
          $ok = $true
          break
        }
      }
      catch {
        # Service might still be restarting; retry.
      }
      Start-Sleep -Seconds 1
    }
    if (!$ok) {
      throw "login failed after retries (last response: $lastResp)"
    }

    $code = (curl.exe -k -s -b $cookie.FullName -o NUL -w "%{http_code}" "$BaseUrl/api/application/version").Trim()
    if ($code -ne "200") {
      throw "expected 200 from $BaseUrl/api/application/version, got $code"
    }
  }
  finally {
    Remove-Item -Force $cookie.FullName -ErrorAction SilentlyContinue
  }
}

if ($RunE2E) {
  $RepoRoot = Split-Path -Parent $PSScriptRoot
  $E2E = Join-Path $RepoRoot "e2e"
  if (-not $env:NANOKVM_BASE_URL) {
    $env:NANOKVM_BASE_URL = $BaseUrl
  }
  if (-not $env:NANOKVM_USER) {
    $env:NANOKVM_USER = $User
  }
  if (-not $env:NANOKVM_PASS) {
    $env:NANOKVM_PASS = $Pass
  }
  Push-Location $E2E
  try {
    npm test
  }
  finally {
    Pop-Location
  }
}

Write-Host "OK"