#!/bin/sh
# Sign a local build so the macOS keychain keeps "Always Allow" across rebuilds.
#
# The keychain checks two things: the app's code requirement and its partition. An app signed
# with an Apple-issued certificate gets a stable partition (its Team ID); anything else is
# partitioned by the hash of each build, so every rebuild asks again. Use an "Apple Development"
# certificate (free with an Apple ID in Xcode → Settings → Accounts → Manage Certificates).
#
# Usage: scripts/dev-sign.sh [binary]   (default: target/release/loa)
# LOA_SIGN_IDENTITY overrides the identity (name or SHA-1 from `security find-identity`).
set -eu
cd "$(dirname "$0")/.."
BIN="${1:-target/release/loa}"

IDENTITY="${LOA_SIGN_IDENTITY:-}"
if [ -z "$IDENTITY" ]; then
  IDENTITY=$(security find-identity -v -p codesigning | awk '/"Apple Development: / {print $2; exit}')
fi
if [ -z "$IDENTITY" ]; then
  echo "Apple Development の署名用証明書が見つかりません（Xcode → 設定 → アカウント → 証明書を管理 で作成）" >&2
  exit 1
fi

codesign --force --sign "$IDENTITY" --identifier loa --timestamp=none "$BIN"
codesign --verify "$BIN"
TEAM=$(codesign -dv "$BIN" 2>&1 | sed -n 's/^TeamIdentifier=//p')
echo "signed: $BIN (team: $TEAM)"
