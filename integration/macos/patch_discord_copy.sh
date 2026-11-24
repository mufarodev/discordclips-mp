#!/usr/bin/env bash
set -euo pipefail
export PATH="/opt/homebrew/bin:$HOME/.cargo/bin:$PATH"

# Build clipsd and create a patched *copy* of Discord Canary.app that loads our
# macOS discord_clips module and clips_bridge.js via Discord's built-in
# `standaloneModules` + `disableUpdater` mechanism.
#
# NEVER touches the installed /Applications/Discord*.app or ~/Library/Application Support/discord*.
#
# Usage:
#   ./integration/macos/patch_discord_copy.sh [DEST_APP_PATH] [SOURCE_APP_VERSION_DIR]

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DEST_APP="${1:-$HOME/Applications/Discord Canary Clips.app}"

# Refuse to overwrite the real installed app or Application Support.
case "$DEST_APP" in
  "/Applications/Discord"*.app|"$HOME/Library/Application Support/discord"*)
    echo "error: refusing to patch installed Discord path: $DEST_APP" >&2
    exit 1
    ;;
esac

# Locate the latest Canary app-<ver> directory containing the Electron .app + modules/.
if [[ -n "${2:-}" ]]; then
  SRC_VER_DIR="$2"
else
  SRC_VER_DIR="$(ls -d "$HOME/Library/Application Support/discordcanary"/app-* 2>/dev/null | sort -V | tail -n 1 || true)"
fi

if [[ -z "$SRC_VER_DIR" || ! -d "$SRC_VER_DIR/Discord Canary.app" || ! -d "$SRC_VER_DIR/modules" ]]; then
  echo "error: could not find Discord Canary app-<ver> with Discord Canary.app and modules/." >&2
  echo "       pass it explicitly as arg 2 if installed elsewhere." >&2
  exit 1
fi

echo "==> Building clipsd (release)..."
cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml" -p clipsd-macos --bin clipsd
CLIPSD_BIN="$REPO_ROOT/target/release/clipsd"
if [[ ! -x "$CLIPSD_BIN" ]]; then
  echo "error: clipsd binary not found at $CLIPSD_BIN" >&2
  exit 1
fi

clone_tree() {
  # Use APFS copy-on-write clone (`cp -cR`) when available so the copied app bundle
  # and modules consume almost zero extra disk space; fall back to `ditto`.
  cp -cR "$1" "$2" 2>/dev/null || ditto "$1" "$2"
}

echo "==> Copying Electron bundle from:"
echo "    $SRC_VER_DIR/Discord Canary.app"
echo "    -> $DEST_APP"
mkdir -p "$(dirname "$DEST_APP")"
rm -rf "$DEST_APP"
clone_tree "$SRC_VER_DIR/Discord Canary.app" "$DEST_APP"

RES="$DEST_APP/Contents/Resources"
MODS="$RES/standalone_modules"
mkdir -p "$MODS"

echo "==> Populating Contents/Resources/standalone_modules/..."
for ver_dir in "$SRC_VER_DIR/modules"/*; do
  [[ -d "$ver_dir" ]] || continue
  for inner in "$ver_dir"/*; do
    [[ -d "$inner" ]] || continue
    mod_name="$(basename "$inner")"
    rm -rf "$MODS/$mod_name"
    clone_tree "$inner" "$MODS/$mod_name"
  done
done

echo "==> Installing discord_clips module + clipsd binary..."
COMMON_DIR="$REPO_ROOT/integration/common"
rm -rf "$MODS/discord_clips"
mkdir -p "$MODS/discord_clips"
cp "$COMMON_DIR/discord_clips/package.json" "$MODS/discord_clips/package.json"
cp "$COMMON_DIR/discord_clips/index.js" "$MODS/discord_clips/index.js"
cp "$CLIPSD_BIN" "$MODS/discord_clips/clipsd"
cp "$CLIPSD_BIN" "$MODS/discord_clips/discord_clips"
chmod +x "$MODS/discord_clips/clipsd" "$MODS/discord_clips/discord_clips"

echo "==> Installing clips_bridge.js into standalone_modules/discord_voice/..."
VOICE_DIR="$MODS/discord_voice"
if [[ ! -f "$VOICE_DIR/index.js" ]]; then
  echo "error: expected $VOICE_DIR/index.js" >&2
  exit 1
fi
cp "$COMMON_DIR/clips_bridge.js" "$VOICE_DIR/clips_bridge.js"

python3 - "$VOICE_DIR/index.js" <<'PY'
import sys
p = sys.argv[1]
s = open(p, 'r', encoding='utf-8').read()
hook = "require('./clips_bridge')(VoiceEngine, features);\n"
if hook not in s:
    target = "module.exports = VoiceEngine;"
    assert target in s, "could not find module.exports = VoiceEngine; in discord_voice/index.js"
    s = s.replace(target, hook + target, 1)
    open(p, 'w', encoding='utf-8').write(s)
PY

echo "==> Stubbing discord_krisp/index.js in the ad-hoc copy..."
# discord_krisp.node verifies the app bundle's Apple Developer ID certificate chain via
# SecCodeCopySigningInformation(kSecCodeInfoCertificates) and dereferences NULL (SIGSEGV)
# when the bundle is ad-hoc signed. Stub it in the standalone copy so the renderer stays stable.
if [[ -d "$MODS/discord_krisp" ]]; then
  cat > "$MODS/discord_krisp/index.js" <<'JS'
"use strict";
module.exports = {
  _initialize() {},
  getNcModels: () => Promise.resolve([]),
  getVadModels: () => Promise.resolve([]),
  getNcModelFilename: () => Promise.resolve(""),
};
JS
fi

echo "==> Updating Contents/Resources/build_info.json (disableUpdater + standaloneModules)..."
python3 - "$RES/build_info.json" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p, 'r', encoding='utf-8'))
d['disableUpdater'] = True
d['standaloneModules'] = True
json.dump(d, open(p, 'w', encoding='utf-8'), indent=2)
PY

echo "==> Updating bundle identity in Contents/Info.plist (separate TCC entry)..."
PLIST="$DEST_APP/Contents/Info.plist"
# Keep CFBundleName ("Discord Canary") intact so Electron finds Contents/Frameworks/Discord Canary Helper*.app.
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier com.hnc.DiscordCanary.clipsd" "$PLIST"
/usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName Discord Canary Clips" "$PLIST" 2>/dev/null || \
  /usr/libexec/PlistBuddy -c "Add :CFBundleDisplayName string Discord Canary Clips" "$PLIST"

# Remove provisioning profile from the copy (incompatible with ad-hoc signature).
rm -f "$DEST_APP/Contents/embedded.provisionprofile"

echo "==> Stripping quarantine and ad-hoc signing the copy..."
chmod -R u+w "$DEST_APP"
xattr -cr "$DEST_APP"

ENT="$(mktemp -t clipsd-entitlements.XXXXXX.plist)"
trap 'rm -f "$ENT"' EXIT
cat > "$ENT" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.cs.allow-jit</key>
  <true/>
  <key>com.apple.security.cs.allow-unsigned-executable-memory</key>
  <true/>
  <key>com.apple.security.cs.disable-library-validation</key>
  <true/>
  <key>com.apple.security.device.audio-input</key>
  <true/>
  <key>com.apple.security.device.camera</key>
  <true/>
</dict>
</plist>
XML

# Sign our clipsd binaries, then helper apps, then the outer bundle ad-hoc.
codesign --force --sign - "$MODS/discord_clips/clipsd"
codesign --force --sign - "$MODS/discord_clips/discord_clips"
for helper in "$DEST_APP/Contents/Frameworks/"*.app; do
  [[ -d "$helper" ]] || continue
  codesign --force --sign - --entitlements "$ENT" "$helper"
done
codesign --force --sign - --entitlements "$ENT" "$DEST_APP"

echo ""
echo "Done! Patched copy created at:"
echo "  $DEST_APP"
echo ""
echo "Launch:"
echo "  open \"$DEST_APP\""
echo "Or run from terminal with console logs (pass --multi-instance if Canary is already open):"
echo "  \"$DEST_APP/Contents/MacOS/Discord Canary\" --multi-instance --enable-logging=stderr"
echo ""
echo "Note: macOS will prompt for Screen & System Audio Recording permission for"
echo "'Discord Canary Clips' (separate from your installed Discord Canary)."
