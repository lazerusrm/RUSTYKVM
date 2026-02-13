param(
  [string]$BaseUrl = "https://192.168.0.49",
  [switch]$RunE2E
)

$ErrorActionPreference = "Stop"

function Assert-Http200([string]$Url) {
  $code = (curl.exe -k -s -o NUL -w "%{http_code}" $Url).Trim()
  if ($code -ne "200") {
    throw "expected 200 from $Url, got $code"
  }
}

Assert-Http200 "$BaseUrl/health"
Assert-Http200 "$BaseUrl/login.html"
Assert-Http200 "$BaseUrl/api/system/capabilities"

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
