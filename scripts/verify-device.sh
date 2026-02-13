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

if [ -n "${NANOKVM_USER:-}" ] && [ -n "${NANOKVM_PASS:-}" ]; then
  COOKIE_JAR="$(mktemp)"
  trap 'rm -f "$COOKIE_JAR"' EXIT
  curl -k -s -c "$COOKIE_JAR" -H "Content-Type: application/json" \
    -d "{\"username\":\"$NANOKVM_USER\",\"password\":\"$NANOKVM_PASS\"}" \
    "$BASE_URL/api/login" >/dev/null
  code="$(curl -k -s -b "$COOKIE_JAR" -o /dev/null -w "%{http_code}" "$BASE_URL/api/application/version" || true)"
  if [ "$code" != "200" ]; then
    echo "expected 200 from $BASE_URL/api/application/version, got $code" >&2
    exit 1
  fi
fi

echo "OK"
