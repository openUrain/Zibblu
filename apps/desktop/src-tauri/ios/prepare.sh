#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."

if [[ ! -f src-tauri/gen/apple/project.yml ]]; then
  npm run tauri -- ios init "$@"
fi

icons=$(mktemp -d)
trap 'rm -rf "$icons"' EXIT
npm run tauri -- icon src-tauri/icons/icon.png --ios-color '#6343aa' --output "$icons"
cp "$icons"/ios/*.png src-tauri/gen/apple/Assets.xcassets/AppIcon.appiconset/
cp src-tauri/ios/ZibbluNative.swift src-tauri/ios/PrivacyInfo.xcprivacy src-tauri/gen/apple/Sources/

project=src-tauri/gen/apple/project.yml
python3 - "$project" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
spec = path.read_text()
marker = "\ntargets:\n"
old_family = '    TARGETED_DEVICE_FAMILY: "1"'
family = '    TARGETED_DEVICE_FAMILY: "1,2"'
if old_family in spec:
    path.write_text(spec.replace(old_family, family))
elif family not in spec:
    assert spec.count(marker) == 1
    path.write_text(spec.replace(marker, "\nsettings:\n  base:\n    SWIFT_VERSION: '5.0'\n" + family + "\n" + marker))
PY
xcodegen generate --spec "$project"
