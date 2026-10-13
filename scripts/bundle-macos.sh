#!/usr/bin/env bash
# Builds target/release/Seesvee.app (registered to open CSV and TSV files) and
# target/release/Seesvee-macos-arm64.zip for distribution.
#
#   ./scripts/bundle-macos.sh            # build the bundle and zip
#   ./scripts/bundle-macos.sh --install  # also copy it to /Applications and open it
set -euo pipefail
cd "$(dirname "$0")/.."

INSTALL=0
if [[ "${1:-}" == "--install" ]]; then
  INSTALL=1
  shift
fi

cargo build --release "$@"

APP=target/release/Seesvee.app
ARCH=$(uname -m)
ZIP=target/release/Seesvee-macos-${ARCH}.zip
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
rm -rf "$APP" "$ZIP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/seesvee "$APP/Contents/MacOS/Seesvee"
cp assets/Seesvee.icns "$APP/Contents/Resources/Seesvee.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Seesvee</string>
  <key>CFBundleDisplayName</key><string>Seesvee</string>
  <key>CFBundleIdentifier</key><string>dev.seesvee.app</string>
  <key>CFBundleExecutable</key><string>Seesvee</string>
  <key>CFBundleIconFile</key><string>Seesvee</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>CSV Document</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>CFBundleTypeIconFile</key><string>Seesvee</string>
      <key>LSHandlerRank</key><string>Owner</string>
      <key>LSItemContentTypes</key>
      <array><string>public.comma-separated-values-text</string></array>
      <key>CFBundleTypeExtensions</key>
      <array><string>csv</string></array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>Delimited Text Document</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.tab-separated-values-text</string>
        <string>public.delimited-values-text</string>
      </array>
      <key>CFBundleTypeExtensions</key>
      <array><string>tsv</string><string>tab</string><string>psv</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST

# Ad-hoc signature so the app runs on Apple Silicon. It is not notarized.
codesign --force --deep --sign - "$APP"
ditto -c -k --keepParent "$APP" "$ZIP"
echo "Built $APP and $ZIP"

if [[ $INSTALL == 1 ]]; then
  rm -rf /Applications/Seesvee.app
  cp -R "$APP" /Applications/
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f /Applications/Seesvee.app
  open /Applications/Seesvee.app
  echo "Installed /Applications/Seesvee.app"
fi
