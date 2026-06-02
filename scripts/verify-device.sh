#!/bin/sh
set -eu

BASE_URL="${NANOKVM_BASE_URL:-http://192.168.0.84}"

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

encrypt_password() {
  password="$1"
  secret_key="$2"
  node -e '
const crypto = require("crypto");
function evp(pass, salt, kl, il) {
  let d = Buffer.alloc(0), b = Buffer.alloc(0);
  while (d.length < kl + il) {
    const h = crypto.createHash("md5");
    if (b.length) h.update(b);
    h.update(pass);
    h.update(salt);
    b = h.digest();
    d = Buffer.concat([d, b]);
  }
  return { key: d.subarray(0, kl), iv: d.subarray(kl, kl + il) };
}
function enc(pw, key) {
  const salt = crypto.randomBytes(8);
  const { key: k, iv } = evp(Buffer.from(key, "utf8"), salt, 32, 16);
  const c = crypto.createCipheriv("aes-256-cbc", k, iv);
  const e = Buffer.concat([c.update(pw, "utf8"), c.final()]);
  return Buffer.concat([Buffer.from("Salted__"), salt, e]).toString("base64");
}
console.log(enc(process.argv[1], process.argv[2]));
' "$password" "$secret_key"
}

check_200 "$BASE_URL/health"
check_200 "$BASE_URL/login.html"
check_200 "$BASE_URL/api/passkey/status"

if [ -n "${NANOKVM_USER:-}" ] && [ -n "${NANOKVM_PASS:-}" ]; then
  if ! command -v node >/dev/null 2>&1; then
    echo "node is required to encrypt the login password" >&2
    exit 1
  fi

  COOKIE_JAR="$(mktemp)"
  trap 'rm -f "$COOKIE_JAR"' EXIT

  KEY_RESP="$(curl -k -s "$BASE_URL/api/auth/encryption-key")"
  SECRET_KEY="$(printf '%s' "$KEY_RESP" | node -e 'let d="";process.stdin.on("data",c=>d+=c);process.stdin.on("end",()=>{const j=JSON.parse(d);if(j.code!==0||!j.data||!j.data.key){process.exit(1)};process.stdout.write(j.data.key);});')"
  if [ -z "$SECRET_KEY" ]; then
    echo "failed to fetch encryption key: $KEY_RESP" >&2
    exit 1
  fi

  ENC_PASS="$(encrypt_password "$NANOKVM_PASS" "$SECRET_KEY")"
  LOGIN_BODY="$(printf '{"username":"%s","password":"%s"}' "$NANOKVM_USER" "$ENC_PASS")"

  ok=0
  i=0
  while [ "$i" -lt 30 ]; do
    RESP="$(curl -k -s -c "$COOKIE_JAR" -H "Content-Type: application/json" \
      --data-binary "$LOGIN_BODY" \
      "$BASE_URL/api/login" || true)"
    CODE="$(printf '%s' "$RESP" | node -e 'let d="";process.stdin.on("data",c=>d+=c);process.stdin.on("end",()=>{try{process.stdout.write(String(JSON.parse(d).code))}catch{process.stdout.write("")}});')"
    if [ "$CODE" = "0" ]; then
      ok=1
      break
    fi
    i=$((i + 1))
    sleep 1
  done
  if [ "$ok" -ne 1 ]; then
    echo "login failed after retries (last response: $RESP)" >&2
    exit 1
  fi

  code="$(curl -k -s -b "$COOKIE_JAR" -o /dev/null -w "%{http_code}" "$BASE_URL/api/application/version" || true)"
  if [ "$code" != "200" ]; then
    echo "expected 200 from $BASE_URL/api/application/version, got $code" >&2
    exit 1
  fi
fi

echo "OK"