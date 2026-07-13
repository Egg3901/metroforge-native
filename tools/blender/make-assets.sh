#!/usr/bin/env bash
# make-assets.sh — regenerate all scripted .glb assets into assets/models/.
#
# Deterministic: each generator is pure bpy geometry (no randomness, no
# timestamps). Re-running reproduces byte-stable geometry. Requires Blender
# 5.x on PATH (headless). Run from anywhere.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
# Bevy's dev asset root is the binary crate's dir (crates/mf-game), so the
# .glb models live under its assets/ tree to be loadable at runtime.
OUT="$REPO/crates/mf-game/assets/models"
BLENDER="${BLENDER:-blender}"

mkdir -p "$OUT"

run() {  # run <script> <outfile> [args...]
  local script="$1"; shift
  local outfile="$1"; shift
  echo ">> $script -> $outfile"
  "$BLENDER" --background --factory-startup --python "$HERE/$script" -- "$@" "$outfile" \
    2>&1 | grep -E '^\[mf_bpy\]|Error|error:' || true
}

# Pilot A — suspension bridge family
"$BLENDER" --background --factory-startup --python "$HERE/gen_bridge.py" -- generic  "$OUT/bridge_suspension.glb" 2>&1 | grep -E '^\[mf_bpy\]|Error' || true
"$BLENDER" --background --factory-startup --python "$HERE/gen_bridge.py" -- brooklyn "$OUT/bridge_brooklyn.glb"   2>&1 | grep -E '^\[mf_bpy\]|Error' || true
# Pilot B — metro consist
"$BLENDER" --background --factory-startup --python "$HERE/gen_train.py"  -- "$OUT/train_metro.glb"   2>&1 | grep -E '^\[mf_bpy\]|Error' || true
# Pilot C — cloud puffs
"$BLENDER" --background --factory-startup --python "$HERE/gen_clouds.py" -- "$OUT/cloud_puffs.glb"   2>&1 | grep -E '^\[mf_bpy\]|Error' || true

echo "== assets =="
ls -la "$OUT"
