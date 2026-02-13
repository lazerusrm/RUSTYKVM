param(
  [string]$BaseUrl = "https://192.168.0.49",
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

Assert-Http200 "$BaseUrl/health"
Assert-Http200 "$BaseUrl/login.html"
Assert-Http200 "$BaseUrl/api/system/capabilities"

if ($User -and $Pass) {
  $cookie = New-TemporaryFile
  try {
    $loginBody = @{ username = $User; password = $Pass } | ConvertTo-Json -Compress
    $resp = curl.exe -k -s -c $cookie.FullName -H "Content-Type: application/json" -d $loginBody "$BaseUrl/api/login"
    $json = $resp | ConvertFrom-Json
    if ($json.code -ne 0) {
      throw "login failed: $($json.msg)"
    }

    Assert-Http200 "$BaseUrl/api/application/version"
  }
  finally {
    Remove-Item -Force $cookie.FullName -ErrorAction SilentlyContinue
  }
}

if ($RunE2E) {
  $RepoRoot = Split-Path -Parent $PSScriptRoot
  $E2E = Join-Path $RepoRoot "e2e"
  Push-Location $E2E
  try {
    npm test
  }
  finally {
    Pop-Location
  }
}

Write-Host "OK"
