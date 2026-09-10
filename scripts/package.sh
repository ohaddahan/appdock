#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
profile=release
case "${1:-}" in
  --debug) profile=debug; cargo build --locked ;;
  '') cargo build --release --locked ;;
  *) echo 'Usage: scripts/package.sh [--debug]' >&2; exit 2 ;;
esac
bundle="$PWD/dist/AppDock.app"
mkdir -p "$bundle/Contents/MacOS"
cp "target/$profile/appdock" "$bundle/Contents/MacOS/AppDock"
cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.appdock.AppDock</string>
<key>CFBundleName</key><string>AppDock</string>
<key>CFBundleDisplayName</key><string>AppDock</string>
<key>CFBundleExecutable</key><string>AppDock</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSPrincipalClass</key><string>NSApplication</string>
<key>NSAccessibilityUsageDescription</key><string>AppDock arranges only the windows you choose to organize and restores their original state when released.</string>
</dict></plist>
PLIST
plutil -lint "$bundle/Contents/Info.plist"
codesign --force --sign - "$bundle"
codesign --verify --strict "$bundle"
echo "$bundle"
