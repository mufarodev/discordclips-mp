#!/usr/bin/env bash
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"

# Vencord-style in-place Linux injector for clipsd.
# Patches ONLY the JS files inside ~/.config/discord*/<ver>/modules/ (or Flatpak config dir)
# and never touches the Discord Electron binary.
#
# Usage:
#   ./integration/linux/inject_discord.sh [--channel canary|ptb|stable|development] [--flatpak] [--data-dir PATH] [--skip-build] [--uninstall]

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMMON_DIR="$REPO_ROOT/integration/common"
CHANNEL="canary"
FLATPAK=0
UNINSTALL=0
SKIP_BUILD=0
CUSTOM_DATA_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --channel) CHANNEL="$2"; shift 2 ;;
    --flatpak) FLATPAK=1; shift ;;
    --data-dir) CUSTOM_DATA_DIR="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help)
      echo "usage: $0 [--channel canary|ptb|stable|development] [--flatpak] [--data-dir PATH] [--skip-build] [--uninstall]"
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [[ -n "$CUSTOM_DATA_DIR" ]]; then
  DATA_DIR="$CUSTOM_DATA_DIR"
else
  XDG_CFG="${XDG_CONFIG_HOME:-$HOME/.config}"
  if [[ "$FLATPAK" -eq 1 ]]; then
    case "$CHANNEL" in
      canary) DATA_DIR="$HOME/.var/app/com.discordapp.DiscordCanary/config/discordcanary" ;;
      ptb) DATA_DIR="$HOME/.var/app/com.discordapp.DiscordPTB/config/discordptb" ;;
      stable) DATA_DIR="$HOME/.var/app/com.discordapp.Discord/config/discord" ;;
      *) echo "error: flatpak channel '$CHANNEL' unsupported (use --data-dir)" >&2; exit 2 ;;
    esac
  else
    case "$CHANNEL" in
      canary) DATA_DIR="$XDG_CFG/discordcanary" ;;
      ptb) DATA_DIR="$XDG_CFG/discordptb" ;;
      development) DATA_DIR="$XDG_CFG/discorddevelopment" ;;
      stable) DATA_DIR="$XDG_CFG/discord" ;;
      *) echo "error: unknown channel '$CHANNEL'" >&2; exit 2 ;;
    esac
  fi
fi

# On Linux Discord uses either `<ver>/modules` (e.g. `0.0.1591/modules`) or `app-<ver>/modules`.
VER_DIR="$(ls -d "$DATA_DIR"/app-* "$DATA_DIR"/0.* 2>/dev/null | sort -V | tail -n 1 || true)"
if [[ -z "$VER_DIR" || ! -d "$VER_DIR/modules" ]]; then
  echo "error: no <ver>/modules directory found in $DATA_DIR" >&2
  exit 1
fi

# Locate discord_desktop_core and discord_voice (handles both new `name-N/name` and legacy `name/` layouts).
find_mod_dir() {
  local name="$1"
  local nested
  nested="$(ls -d "$VER_DIR"/modules/"${name}"-*/"${name}" 2>/dev/null | sort -V | tail -n 1 || true)"
  if [[ -n "$nested" && -d "$nested" ]]; then
    echo "$nested"
  elif [[ -d "$VER_DIR/modules/$name" ]]; then
    echo "$VER_DIR/modules/$name"
  fi
}

CORE_DIR="$(find_mod_dir discord_desktop_core)"
VOICE_DIR="$(find_mod_dir discord_voice)"
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

CLIPSD_BIN="${CLIPSD_BIN:-$REPO_ROOT/target/release/clipsd}"
if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "==> Building clipsd-linux (release)..."
  cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml" -p clipsd-linux --bin clipsd
fi
if [[ ! -x "$CLIPSD_BIN" ]]; then
  echo "error: clipsd binary not found or not executable at $CLIPSD_BIN" >&2
  exit 1
fi

echo "==> Installing local discord_clips module into $CLIPS_PARENT/discord_clips..."
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
echo ""
echo "Note: On Linux, Discord disables the default Alt+C clip keybind — bind one in"
echo "Settings -> Keybinds (Save Clip) or Settings -> Clips after relaunching."
echo "To remove cleanly later:"
echo "  $0 --channel $CHANNEL --uninstall"
