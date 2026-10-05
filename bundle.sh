#!/bin/zsh
# Build "Heyra.app" (macOS) so permissions attach to the app, not the terminal.
set -e
cd "$(dirname "$0")"
cargo build --release
APP="target/Heyra.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/heyra "$APP/Contents/MacOS/heyra"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Heyra</string>
  <key>CFBundleIdentifier</key><string>dev.alcun.heyra</string>
  <key>CFBundleExecutable</key><string>heyra</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSMicrophoneUsageDescription</key><string>Heyra listens while you hold the push-to-talk key, and turns speech into text on this Mac.</string>
</dict></plist>
PLIST
codesign --force --sign - "$APP"
echo "Built $APP"
