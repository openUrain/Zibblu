#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."

if [[ ! -f src-tauri/gen/apple/project.yml ]]; then
  npm run tauri -- ios init "$@"
fi

icons=$(mktemp -d)
trap 'rm -rf "$icons"' EXIT
npm run tauri -- icon src-tauri/icons/icon.png --ios-color '#f5f5f7' --output "$icons"
cp "$icons"/ios/*.png src-tauri/gen/apple/Assets.xcassets/AppIcon.appiconset/
cp src-tauri/ios/ZibbluNative.swift src-tauri/ios/PrivacyInfo.xcprivacy src-tauri/gen/apple/Sources/

project=src-tauri/gen/apple/project.yml
if ! grep -q '^    TARGETED_DEVICE_FAMILY: "1"$' "$project"; then
  python3 - "$project" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
spec = path.read_text()
marker = "\ntargets:\n"
assert spec.count(marker) == 1
path.write_text(spec.replace(marker, "\nsettings:\n  base:\n    SWIFT_VERSION: '5.0'\n    TARGETED_DEVICE_FAMILY: \"1\"\n" + marker))
PY
fi
xcodegen generate --spec "$project"
