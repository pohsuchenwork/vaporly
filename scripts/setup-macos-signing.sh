#!/usr/bin/env bash
# Give local Vaporly builds a STABLE self-signed code-signing identity so macOS
# Accessibility / Input-Monitoring (TCC) grants PERSIST across rebuilds & updates.
#
# Background: TCC ties a grant to the build's code signature. Ad-hoc signing ("-")
# changes the signature every rebuild, so macOS silently drops the grant and the
# user must re-approve. Signing every build with ONE self-signed cert keeps the
# designated requirement stable, so the grant sticks.
#
# ONE-TIME MANUAL STEP (the only part macOS will not let us automate, see
# SETUP.md): making codesign trust a self-signed cert requires a trust-settings
# change, which pops a single "modify Certificate Trust Settings" password
# dialog. Approve it once; everything else here is automatic and idempotent.
#
# Usage: bash scripts/setup-macos-signing.sh
set -euo pipefail

CERT_NAME="Vaporly Local Signing"
KC="${HOME}/Library/Keychains/vaporly-signing.keychain-db"
KC_PASS="vaporly-local"    # local, non-secret: guards a self-signed signing key only
P12_PASS="vaporly"         # transient, for the openssl->security import handoff
CONF="$(cd "$(dirname "$0")/.." && pwd)/src-tauri/tauri.conf.json"

trust_ok() { codesign --version >/dev/null 2>&1 && \
  security find-identity -v -p codesigning "$KC" 2>/dev/null | grep -q "$CERT_NAME"; }

if trust_ok; then
  echo "Trusted signing identity '$CERT_NAME' already set up."
else
  WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
  # 1. Self-signed cert with the Code Signing EKU.
  cat > "$WORK/req.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = v3
prompt = no
[dn]
CN = ${CERT_NAME}
[v3]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF
  openssl req -x509 -newkey rsa:2048 -sha256 -days 3650 -nodes \
    -keyout "$WORK/key.pem" -out "$WORK/cert.pem" -config "$WORK/req.cnf" >/dev/null 2>&1
  # -legacy + -macalg sha1: OpenSSL 3 defaults produce a p12 macOS can't import.
  openssl pkcs12 -export -legacy -macalg sha1 -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
    -name "$CERT_NAME" -out "$WORK/id.p12" -passout "pass:${P12_PASS}" >/dev/null 2>&1

  # 2. Dedicated keychain (known password => codesign never prompts for the key).
  security create-keychain -p "$KC_PASS" "$KC" 2>/dev/null || true
  security set-keychain-settings "$KC"
  security unlock-keychain -p "$KC_PASS" "$KC"
  security import "$WORK/id.p12" -k "$KC" -P "$P12_PASS" -A -T /usr/bin/codesign >/dev/null 2>&1 || true
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KC_PASS" "$KC" >/dev/null 2>&1 || true
  EXISTING="$(security list-keychains -d user | sed 's/[",]//g' | awk '{$1=$1};1')"
  # shellcheck disable=SC2086
  security list-keychains -d user -s "$KC" $EXISTING >/dev/null 2>&1 || true

  # 3. THE ONE MANUAL GATE: trust the cert for code signing (user domain).
  #    macOS shows a single password prompt here, approve it.
  echo ">> Approve the macOS 'Certificate Trust Settings' password prompt now..."
  security add-trusted-cert -r trustRoot -p codeSign -k "$KC" "$WORK/cert.pem"

  trust_ok || { echo "ERROR: identity still not valid after trust, see SETUP.md" >&2; exit 1; }
  echo "Signing identity created and trusted."
fi

# 4. Point local builds at the cert (CI keeps ad-hoc via unsigned.tauri.conf.json).
if command -v jq >/dev/null 2>&1; then
  tmp="$(mktemp)"
  jq --arg id "$CERT_NAME" '.bundle.macOS.signingIdentity = $id' "$CONF" > "$tmp" && mv "$tmp" "$CONF"
  echo "Set tauri.conf.json bundle.macOS.signingIdentity = '$CERT_NAME'."
fi
echo "Done. Rebuild once and grant Accessibility, it will persist from now on."
