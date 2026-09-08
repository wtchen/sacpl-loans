#!/bin/bash
# Build the release DMG with a README next to the app.
#
# Tauri's DMG bundler can't add arbitrary files to the disk image window
# (only the EULA license), so this script:
#   1. builds the .app bundle via `npm run tauri build -- --bundles app`
#   2. stages the .app together with dmg/README.txt and LICENSE
#   3. creates the DMG with hdiutil
#   4. re-attaches the license agreement with hdiutil udifrez — the same
#      mechanism Tauri's own DMG step uses (see scripts/eula-resources-template.xml)
#
# Output: src-tauri/target/release/bundle/dmg/SacPL Loans_<version>_<arch>.dmg

set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(python3 -c "import json;print(json.load(open('src-tauri/tauri.conf.json'))['version'])")
ARCH=$(uname -m)
APP="src-tauri/target/release/bundle/macos/SacPL Loans.app"
OUT_DIR="src-tauri/target/release/bundle/dmg"
OUT="$OUT_DIR/SacPL Loans_${VERSION}_${ARCH}.dmg"

# 1) Build just the app bundle (Tauri's DMG step is skipped).
npm run tauri build -- --bundles app

# 2) Stage the DMG contents.
STAGED=$(mktemp -d)
trap 'rm -rf "$STAGED"' EXIT
cp -R "$APP" "$STAGED/"
cp dmg/README.txt "$STAGED/How to Install.txt"
cp LICENSE "$STAGED/LICENSE.txt"

# 3) Build the disk image.
mkdir -p "$OUT_DIR"
rm -f "$OUT"
hdiutil create -volname "SacPL Loans" -srcfolder "$STAGED" -format UDZO -o "$OUT"

# 4) Attach the EULA license agreement (opens with an Agree/Disagree prompt).
EULA_DATA=$(openssl base64 -in LICENSE | tr -d '\n' | awk '{gsub(/.{52}/,"&\n")}1')
EULA_XML=$(mktemp -t eula-XXXXXXXX.xml)
EULA_DATA="$EULA_DATA" TMPFILE="$EULA_XML" python3 -c '
import os
with open("scripts/eula-resources-template.xml") as f:
    tpl = f.read()
with open(os.environ["TMPFILE"], "w") as f:
    f.write(tpl.replace("${EULA_DATA}", os.environ["EULA_DATA"]).replace("${EULA_FORMAT}", "TEXT"))
'
hdiutil udifrez -xml "$EULA_XML" '' -quiet "$OUT"

echo "DMG ready: $OUT"
ls -lh "$OUT"