#!/usr/bin/env bash
set -euo pipefail
export PATH="/opt/homebrew/bin:$HOME/.cargo/bin:$PATH"

# Vencord-style in-place macOS injector for clipsd.
# Patches ONLY the JS files inside ~/Library/Application Support/discord*/app-<ver>/modules/
# and NEVER touches Discord*.app — keeping Discord's genuine Apple-notarized signature
# intact (so discord_krisp.node and existing TCC permissions keep working normally).
#
# Usage:
#   ./integration/macos/inject_discord.sh [--channel canary|ptb|stable|development] [--uninstall]

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CHANNEL="canary"
UNINSTALL=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --channel) CHANNEL="$2"; shift 2 ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help)
      echo "usage: $0 [--channel canary|ptb|stable|development] [--uninstall]"
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

case "$CHANNEL" in
  canary) DATA_DIR="$HOME/Library/Application Support/discordcanary" ;;
  ptb) DATA_DIR="$HOME/Library/Application Support/discordptb" ;;
  development) DATA_DIR="$HOME/Library/Application Support/discorddevelopment" ;;
  stable) DATA_DIR="$HOME/Library/Application Support/discord" ;;
  *) echo "error: unknown channel '$CHANNEL'" >&2; exit 2 ;;
esac

VER_DIR="$(ls -d "$DATA_DIR"/app-* 2>/dev/null | sort -V | tail -n 1 || true)"
if [[ -z "$VER_DIR" || ! -d "$VER_DIR/modules" ]]; then
  echo "error: no app-<ver>/modules found in $DATA_DIR" >&2
  exit 1
fi

CORE_DIR="$(ls -d "$VER_DIR"/modules/discord_desktop_core-*/discord_desktop_core 2>/dev/null | sort -V | tail -n 1 || true)"
VOICE_DIR="$(ls -d "$VER_DIR"/modules/discord_voice-*/discord_voice 2>/dev/null | sort -V | tail -n 1 || true)"
CLIPS_PARENT="$DATA_DIR/clipsd_modules"

if [[ -z "$CORE_DIR" || -z "$VOICE_DIR" ]]; then
  echo "error: could not locate discord_desktop_core or discord_voice under $VER_DIR/modules" >&2
  exit 1
fi

if [[ "$UNINSTALL" -eq 1 ]]; then
  echo "==> Uninstalling clipsd hooks from $VER_DIR..."
  if [[ -f "$CORE_DIR/index.js.clipsd.bak" ]]; then
    mv "$CORE_DIR/index.js.clipsd.bak" "$CORE_DIR/index.js"
  fi
  rm -f "$CORE_DIR/clipsd_core_hook.js"
  if [[ -f "$VOICE_DIR/index.js.clipsd.bak" ]]; then
    mv "$VOICE_DIR/index.js.clipsd.bak" "$VOICE_DIR/index.js"
  fi
  rm -f "$VOICE_DIR/clips_bridge.js"
  rm -rf "$CLIPS_PARENT"
  echo "Done! Restored original module files. Restart Discord to apply."
  exit 0
fi

echo "==> Building clipsd (release)..."
cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml" -p clipsd-macos --bin clipsd
CLIPSD_BIN="$REPO_ROOT/target/release/clipsd"

echo "==> Installing local discord_clips module into $CLIPS_PARENT/discord_clips..."
COMMON_DIR="$REPO_ROOT/integration/common"
mkdir -p "$CLIPS_PARENT/discord_clips"
cp "$COMMON_DIR/discord_clips/package.json" "$CLIPS_PARENT/discord_clips/package.json"
cp "$COMMON_DIR/discord_clips/index.js" "$CLIPS_PARENT/discord_clips/index.js"
cp "$CLIPSD_BIN" "$CLIPS_PARENT/discord_clips/clipsd"
cp "$CLIPSD_BIN" "$CLIPS_PARENT/discord_clips/discord_clips"
chmod +x "$CLIPS_PARENT/discord_clips/clipsd" "$CLIPS_PARENT/discord_clips/discord_clips"

echo "==> Hooking $CORE_DIR/index.js..."
if [[ ! -f "$CORE_DIR/index.js.clipsd.bak" ]]; then
  cp "$CORE_DIR/index.js" "$CORE_DIR/index.js.clipsd.bak"
fi
cp "$COMMON_DIR/core_hook.js" "$CORE_DIR/clipsd_core_hook.js"
python3 - "$CORE_DIR/index.js" "$CLIPS_PARENT" <<'PY'
import json, sys
p, clips_parent = sys.argv[1], sys.argv[2]
s = open(p, 'r', encoding='utf-8').read()
if "clipsd_core_hook" not in s:
    # Wrap whatever discord_desktop_core/index.js exports (compatible with Vencord/etc.).
    wrapper = (
        f"// clipsd-hook-start\n"
        f"const __clipsdHook = require('./clipsd_core_hook');\n"
        f"__clipsdHook.beforeCoreLoad({json.dumps(clips_parent)});\n"
        f"const __clipsdOrig = (function() {{\n"
        f"  const module = {{ exports: {{}} }};\n"
        f"  const exports = module.exports;\n"
        f"{s}\n"
        f"  return module.exports;\n"
        f"}})();\n"
        f"module.exports = __clipsdHook.afterCoreLoad(__clipsdOrig, {json.dumps(clips_parent)});\n"
        f"// clipsd-hook-end\n"
    )
    open(p, 'w', encoding='utf-8').write(wrapper)
PY

echo "==> Hooking $VOICE_DIR/index.js..."
if [[ ! -f "$VOICE_DIR/index.js.clipsd.bak" ]]; then
  cp "$VOICE_DIR/index.js" "$VOICE_DIR/index.js.clipsd.bak"
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

echo ""
echo "Done! In-place hooks installed for Discord ($CHANNEL) at:"
echo "  $VER_DIR"
echo "Discord's .app bundle and Apple signature were NOT modified."
echo ""
echo "Fully quit and relaunch Discord ($CHANNEL) to load clipsd."
echo "To remove cleanly later:"
echo "  $0 --channel $CHANNEL --uninstall"
