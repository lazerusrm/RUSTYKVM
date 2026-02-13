#!/bin/sh
set -eu

BASE_URL="${NANOKVM_BASE_URL:-https://192.168.0.49}"

check_200() {
  url="$1"
  code="$(curl -k -s -o /dev/null -w "%{http_code}" "$url")"
  if [ "$code" != "200" ]; then
    echo "expected 200 from $url, got $code" >&2
    exit 1
  fi
}

check_200 "$BASE_URL/health"
check_200 "$BASE_URL/login.html"
check_200 "$BASE_URL/api/application/version"

echo "OK"

