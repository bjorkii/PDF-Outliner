#!/usr/bin/env bash
set -euo pipefail

# Builds the ui crate in release mode for <target-triple>, bundles it into
# "PDF Outliner.app" together with the given pdfium dylib, ad-hoc signs it
# (free, no Apple Developer ID required — arm64 refuses to launch any binary
# with zero signature at all), and packs it into a .dmg (drag to Applications,
# plus a first-run note about clearing the quarantine attribute) for distribution.
#
# Usage: package-macos.sh <target-triple> <pdfium-dylib-path> [version-tag]
#   target-triple: aarch64-apple-darwin | x86_64-apple-darwin
#   version-tag: e.g. "v0.1.4" — used in the dmg filename and (with the
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
# 창 제목에 넣을 버전(crates/ui/build.rs) — 인자로 받은 태그만 넘긴다. 인자가 없는 로컬 실행은
# 비워 두어 build.rs가 git describe를 쓰게 한다(Cargo.toml 기반 기본값은 실제 버전과 어긋남).
PDF_OUTLINER_VERSION="${3:-}" cargo build --release --target "$TARGET" -p ui

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
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>
            <string>PDF Document</string>
            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>LSHandlerRank</key>
            <string>Alternate</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>com.adobe.pdf</string>
            </array>
            <key>CFBundleTypeIconFile</key>
            <string>icon.icns</string>
        </dict>
    </array>
</dict>
</plist>
PLIST

echo "==> Ad-hoc signing (no paid Apple Developer ID needed)"
codesign --force --deep --sign - "$APP_DIR"
codesign --verify --verbose "$APP_DIR"

# Distribution format: .dmg (2026-09-14, was .zip). The volume holds the app, an
# Applications symlink (drag-and-drop install) and a first-run note — the app is
# not notarized, so the quarantine attribute must be cleared once before launch.
DMG_NAME="PDF-Outliner-$VERSION_TAG-macos-$ARCH_LABEL.dmg"
STAGING_DIR="$DIST_DIR/dmg-staging"
rm -rf "$STAGING_DIR" "$DIST_DIR/$DMG_NAME"
mkdir -p "$STAGING_DIR"
ditto "$APP_DIR" "$STAGING_DIR/PDF Outliner.app"
ln -s /Applications "$STAGING_DIR/Applications"
cat > "$STAGING_DIR/처음 실행 전에 읽어주세요.txt" <<'NOTE'
PDF Outliner 설치 방법

1. "PDF Outliner" 아이콘을 옆의 "Applications" 폴더로 끌어다 놓으세요.

2. 이 앱은 Apple 개발자 등록 없이 배포되어, 처음 실행하기 전에 한 번만
   터미널(응용 프로그램 > 유틸리티 > 터미널)에 아래 명령을 붙여 넣고 Enter를 누르세요.

   xattr -dr com.apple.quarantine "/Applications/PDF Outliner.app"

3. 그다음부터는 평소처럼 실행하면 됩니다.
NOTE

echo "==> Creating $DMG_NAME"
# hdiutil occasionally fails with "Resource busy" on CI runners — retry a few times.
for attempt in 1 2 3; do
  if hdiutil create -volname "PDF Outliner" -srcfolder "$STAGING_DIR" -ov -format UDZO "$DIST_DIR/$DMG_NAME"; then
    break
  fi
  if [[ "$attempt" == 3 ]]; then
    echo "hdiutil create failed after $attempt attempts" >&2
    exit 1
  fi
  echo "hdiutil create failed (attempt $attempt), retrying..." >&2
  sleep 5
done
rm -rf "$STAGING_DIR"

echo "==> Done: $DIST_DIR/$DMG_NAME"
