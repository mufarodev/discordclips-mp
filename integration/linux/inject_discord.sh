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
LINUX_DIR="$REPO_ROOT/integration/linux"
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
# The updater may stage a newer app-<ver> next to the running one and switch on restart
# (see install-state/app.toml), so we hook *every* version dir that has the modules.
mapfile -t VER_DIRS < <(ls -d "$DATA_DIR"/app-* "$DATA_DIR"/0.* 2>/dev/null | sort -V || true)
HAVE_MODULES=()
for d in "${VER_DIRS[@]}"; do [[ -d "$d/modules" ]] && HAVE_MODULES+=("$d"); done
if [[ ${#HAVE_MODULES[@]} -eq 0 ]]; then
  echo "error: no <ver>/modules directory found in $DATA_DIR" >&2
  exit 1
fi

# Locate a module dir inside a version dir (handles both `name-N/name` and legacy `name/` layouts).
find_mod_dir() {
  local ver="$1" name="$2" nested
  nested="$(ls -d "$ver"/modules/"${name}"-*/"${name}" 2>/dev/null | sort -V | tail -n 1 || true)"
  if [[ -n "$nested" && -d "$nested" ]]; then
    echo "$nested"
  elif [[ -d "$ver/modules/$name" ]]; then
    echo "$ver/modules/$name"
  fi
}

CLIPS_PARENT="$DATA_DIR/clipsd_modules"

uninstall_ver() {
  local ver="$1" core voice utils
  core="$(find_mod_dir "$ver" discord_desktop_core)"; voice="$(find_mod_dir "$ver" discord_voice)"; utils="$(find_mod_dir "$ver" discord_utils)"
  [[ -z "$core" || -z "$voice" ]] && return 0
  echo "==> Uninstalling clipsd hooks from $ver..."
  [[ -f "$core/index.js.clipsd.bak" ]] && mv "$core/index.js.clipsd.bak" "$core/index.js"
  rm -f "$core/clipsd_core_hook.js"
  [[ -f "$voice/index.js.clipsd.bak" ]] && mv "$voice/index.js.clipsd.bak" "$voice/index.js"
  rm -f "$voice/clips_bridge.js"
  if [[ -n "$utils" ]]; then
    [[ -f "$utils/index.js.clipsd.bak" ]] && mv "$utils/index.js.clipsd.bak" "$utils/index.js"
    rm -f "$utils/clipsd_utils_hook.js"
  fi
}

if [[ "$UNINSTALL" -eq 1 ]]; then
  for ver in "${HAVE_MODULES[@]}"; do uninstall_ver "$ver"; done
  rm -rf "$CLIPS_PARENT"
  rm -f "${XDG_DATA_HOME:-$HOME/.local/share}/vulkan/implicit_layer.d/VK_LAYER_CLIPSD_capture.json"
  rm -f "$HOME/.local/share/clipsd/libclipsd_vk_layer.so"
  echo "Done! Restored original module files and removed Vulkan layer. Restart Discord to apply."
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
if command -v gcc >/dev/null 2>&1 && [[ -f "$REPO_ROOT/crates/linux/vk_layer/build.sh" ]]; then
  echo "==> Building & installing implicit Vulkan DMA-BUF capture layer..."
  bash "$REPO_ROOT/crates/linux/vk_layer/build.sh"
fi

echo "==> Installing local discord_clips module into $CLIPS_PARENT/discord_clips..."
mkdir -p "$CLIPS_PARENT/discord_clips"
cp "$COMMON_DIR/discord_clips/package.json" "$CLIPS_PARENT/discord_clips/package.json"
cp "$COMMON_DIR/discord_clips/index.js" "$CLIPS_PARENT/discord_clips/index.js"
cp "$CLIPSD_BIN" "$CLIPS_PARENT/discord_clips/clipsd"
cp "$CLIPSD_BIN" "$CLIPS_PARENT/discord_clips/discord_clips"
chmod +x "$CLIPS_PARENT/discord_clips/clipsd" "$CLIPS_PARENT/discord_clips/discord_clips"


install_ver() {
  local ver="$1" CORE_DIR VOICE_DIR
  CORE_DIR="$(find_mod_dir "$ver" discord_desktop_core)"
  VOICE_DIR="$(find_mod_dir "$ver" discord_voice)"
  if [[ -z "$CORE_DIR" || -z "$VOICE_DIR" ]]; then
    echo "==> skipping $ver (discord_desktop_core / discord_voice not found)"
    return 0
  fi
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

  local UTILS_DIR
  UTILS_DIR="$(find_mod_dir "$ver" discord_utils)"
  if [[ -n "$UTILS_DIR" && -f "$UTILS_DIR/index.js" ]]; then
    echo "==> Hooking $UTILS_DIR/index.js..."
    if [[ ! -f "$UTILS_DIR/index.js.clipsd.bak" ]]; then
      cp "$UTILS_DIR/index.js" "$UTILS_DIR/index.js.clipsd.bak"
    fi
    cp "$LINUX_DIR/utils_hook.js" "$UTILS_DIR/clipsd_utils_hook.js"
    python3 - "$UTILS_DIR/index.js" <<'PY'
import sys
p = sys.argv[1]
s = open(p, 'r', encoding='utf-8').read()
hook = "require('./clipsd_utils_hook')(nativeUtils);\n"
if hook not in s:
    target = "module.exports = nativeUtils;"
    assert target in s, "could not find module.exports = nativeUtils; in discord_utils/index.js"
    s = s.replace(target, hook + target, 1)
    open(p, 'w', encoding='utf-8').write(s)
PY
  fi

}

for ver in "${HAVE_MODULES[@]}"; do install_ver "$ver"; done

echo ""
echo "Done! In-place hooks installed for Discord ($CHANNEL) at:"
printf '  %s\n' "${HAVE_MODULES[@]}"
echo ""
echo "Note: On Linux, Discord disables the default Alt+C clip keybind — bind one in"
echo "Settings -> Keybinds (Save Clip) or Settings -> Clips after relaunching."
echo "To remove cleanly later:"
echo "  $0 --channel $CHANNEL --uninstall"
