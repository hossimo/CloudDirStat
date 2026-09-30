#!/usr/bin/env bash
# Builds CloudDirStat.app (one universal app for Apple Silicon and Intel Macs) and a
# DMG to download it in. Runs on macOS: it needs lipo, codesign, and hdiutil.
#
# Usage: scripts/macos_app.sh VERSION ARM64_GUI X86_64_GUI OUT_DMG
#   VERSION     e.g. 1.2.0 (no leading "v")
#   ARM64_GUI   clouddirstat-gui built for aarch64-apple-darwin
#   X86_64_GUI  clouddirstat-gui built for x86_64-apple-darwin
#   OUT_DMG     the DMG file to write
#
# The app gets an ad-hoc signature, which needs no Apple account. Without any
# signature, macOS on Apple Silicon reports downloaded apps as "damaged". Real code
# signing (a Developer ID) and notarization replace the codesign step later.
set -euo pipefail

if [[ $# -ne 4 ]]; then
  sed -n '5,10p' "$0"
  exit 2
fi
version="$1"
arm64_gui="$2"
x86_64_gui="$3"
dmg="$4"

name="CloudDirStat"
bundle_id="com.downrighttech.clouddirstat"
executable="clouddirstat-gui"
icon="$(dirname "$0")/../icons/clouddirstat.icns"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
app="$work/staging/$name.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

# One executable with both architectures in it.
lipo -create -output "$app/Contents/MacOS/$executable" "$arm64_gui" "$x86_64_gui"
# Build artifacts lose their executable bit on the way between CI jobs.
chmod 755 "$app/Contents/MacOS/$executable"
cp "$icon" "$app/Contents/Resources/clouddirstat.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>$name</string>
    <key>CFBundleDisplayName</key>
    <string>$name</string>
    <key>CFBundleIdentifier</key>
    <string>$bundle_id</string>
    <key>CFBundleExecutable</key>
    <string>$executable</string>
    <key>CFBundleIconFile</key>
    <string>clouddirstat</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.utilities</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.12</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>Dual-licensed under MIT or Apache-2.0</string>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist"

codesign --force --deep --sign - "$app"
codesign --verify --deep --strict --verbose=2 "$app"
echo "architectures: $(lipo -archs "$app/Contents/MacOS/$executable")"
# Starts the app far enough to print its version (no window), to catch a broken build.
"$app/Contents/MacOS/$executable" --version

# The DMG window shows the app next to a shortcut to Applications to drag it onto.
ln -s /Applications "$work/staging/Applications"
mkdir -p "$(dirname "$dmg")"
hdiutil create -volname "$name" -srcfolder "$work/staging" -ov -format UDZO "$dmg"
echo "wrote $dmg"
