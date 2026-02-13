# NanoKVM E2E (Playwright)

These are smoke tests that run against an already-deployed NanoKVM instance (for example your device at `192.168.0.49`).

## Setup

```powershell
cd C:\Users\Administrator\Documents\nanokvm\nanokvm-rs\e2e
npm install
npx playwright install
```

## Run

Defaults:
- `NANOKVM_BASE_URL`: `https://192.168.0.49`
- `NANOKVM_USER`: `admin`
- `NANOKVM_PASS`: `admin`

```powershell
cd C:\Users\Administrator\Documents\nanokvm\nanokvm-rs\e2e
npm test
```

Override target:

```powershell
$env:NANOKVM_BASE_URL = "https://192.168.0.49"
$env:NANOKVM_USER = "admin"
$env:NANOKVM_PASS = "admin"
npm test
```

