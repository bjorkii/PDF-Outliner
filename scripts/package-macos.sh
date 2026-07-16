#!/usr/bin/env bash
set -euo pipefail

# Builds the ui crate in release mode for <target-triple>, bundles it into
# "PDF Outliner.app" together with the given pdfium dylib, ad-hoc signs it
# (free, no Apple Developer ID required — arm64 refuses to launch any binary
# with zero signature at all), and zips the result for distribution.
#
# Usage: package-macos.sh <target-triple> <pdfium-dylib-path> [version-tag]
#   target-triple: aarch64-apple-darwin | x86_64-apple-darwin
#   version-tag: e.g. "v0.1.4" — used in the zip filename and (with the
#     leading "v" stripped) in Info.plist. Defaults to "v<Cargo.toml version>"
#     for convenient local/ad-hoc runs; CI always passes the actual release
#     tag (the git tag is the single source of truth for release versions —
#     Cargo.toml's version is not bumped per release and will drift).

TARGET="${1:?usage: package-macos.sh <target-triple> <pdfium-dylib-path> [version-tag]}"
PDFIUM_DYLIB="${2:?usage: package-macos.sh <target-triple> <pdfium-dylib-path> [version-tag]}"
VERSION_TAG="${3:-}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

if [[ ! -f "$PDFIUM_DYLIB" ]]; then
  echo "pdfium dylib not found: $PDFIUM_DYLIB" >&2
  exit 1
fi

if [[ -z "$VERSION_TAG" ]]; then
  VERSION_TAG="v$(grep -m1 '^version = ' Cargo.toml | sed -E 's/version = "(.*)"/\1/')"
fi
VERSION="${VERSION_TAG#v}"

case "$TARGET" in
  aarch64-apple-darwin) ARCH_LABEL="arm64" ;;
  x86_64-apple-darwin) ARCH_LABEL="x64" ;;
  *)
    echo "unsupported target: $TARGET (expected aarch64-apple-darwin or x86_64-apple-darwin)" >&2
    exit 1
    ;;
esac

echo "==> Building PDF-Outliner release binary for $TARGET"
cargo build --release --target "$TARGET" -p ui

DIST_DIR="$REPO_ROOT/dist"
APP_DIR="$DIST_DIR/PDF Outliner.app"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Frameworks" "$APP_DIR/Contents/Resources"

cp "target/$TARGET/release/PDF-Outliner" "$APP_DIR/Contents/MacOS/PDF-Outliner"
cp "$PDFIUM_DYLIB" "$APP_DIR/Contents/Frameworks/libpdfium.dylib"
cp "assets/icon/icon.icns" "$APP_DIR/Contents/Resources/icon.icns"

cat > "$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>PDF Outliner</string>
    <key>CFBundleDisplayName</key>
    <string>PDF Outliner</string>
    <key>CFBundleIdentifier</key>
    <string>com.pdfoutliner.app</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleExecutable</key>
    <string>PDF-Outliner</string>
    <key>CFBundleIconFile</key>
    <string>icon.icns</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

echo "==> Ad-hoc signing (no paid Apple Developer ID needed)"
codesign --force --deep --sign - "$APP_DIR"
codesign --verify --verbose "$APP_DIR"

ZIP_NAME="PDF-Outliner-$VERSION_TAG-macos-$ARCH_LABEL.zip"
rm -f "$DIST_DIR/$ZIP_NAME"
ditto -c -k --sequesterRsrc --keepParent "$APP_DIR" "$DIST_DIR/$ZIP_NAME"

echo "==> Done: $DIST_DIR/$ZIP_NAME"
