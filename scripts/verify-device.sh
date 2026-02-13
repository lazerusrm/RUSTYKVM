#!/bin/sh
set -eu

BASE_URL="${NANOKVM_BASE_URL:-https://192.168.0.49}"

check_200() {
  url="$1"
  i=0
  code=""
  while [ "$i" -lt 30 ]; do
    code="$(curl -k -s -o /dev/null -w "%{http_code}" "$url" || true)"
    if [ "$code" = "200" ]; then
      return 0
    fi
    i=$((i + 1))
    sleep 1
  done
  echo "expected 200 from $url, got $code" >&2
  exit 1
}

check_200 "$BASE_URL/health"
check_200 "$BASE_URL/login.html"
check_200 "$BASE_URL/api/system/capabilities"

echo "OK"
