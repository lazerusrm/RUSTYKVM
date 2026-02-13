param(
  [string]$BaseUrl = "https://192.168.0.49",
  [switch]$RunE2E
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
