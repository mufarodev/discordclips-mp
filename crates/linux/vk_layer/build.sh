#!/usr/bin/env bash
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
OUT_DIR="${1:-$HOME/.local/share/clipsd}"
LAYER_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/vulkan/implicit_layer.d"
LIB_PATH="$OUT_DIR/libclipsd_vk_layer.so"

mkdir -p "$OUT_DIR" "$LAYER_DIR"
gcc -O2 -Wall -Wextra -fPIC -shared \
  -o "$LIB_PATH" \
  "$DIR/clipsd_vk_layer.c" \
  -lpthread

sed "s|@LIB_PATH@|$LIB_PATH|g" "$DIR/VK_LAYER_CLIPSD_capture.json.in" \
  > "$LAYER_DIR/VK_LAYER_CLIPSD_capture.json"

echo "Built & installed Vulkan implicit layer:"
echo "  lib:      $LIB_PATH"
echo "  manifest: $LAYER_DIR/VK_LAYER_CLIPSD_capture.json"
