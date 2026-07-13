#!/usr/bin/env bash
# make-assets.sh — regenerate all scripted .glb assets into assets/models/.
#
# Deterministic: each generator is pure bpy geometry (no randomness, no
# timestamps). Re-running reproduces byte-stable geometry. Requires Blender
# 5.x on PATH (headless). Run from anywhere.
#
#   ./tools/blender/make-assets.sh                # regenerate the .glb assets
#   ./tools/blender/make-assets.sh --preview      # ALSO render turntable sheets
#                                                 # into tools/blender/previews/
#
# The --preview flag drives the model-craft critique loop (ops-knowledge doc
# `metroforge-model-craft-method`): each generator renders 6 flat-shaded views
# (front/side/3quarter/top + game-camera oblique + ~2km far) so the silhouette
# checklist can be scored from the images BEFORE the .glb is trusted.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
OUT="$REPO/crates/mf-game/assets/models"
PREV="$HERE/previews"
BLENDER="${BLENDER:-blender}"

PREVIEW=0
if [[ "${1:-}" == "--preview" ]]; then PREVIEW=1; fi

mkdir -p "$OUT"
if [[ $PREVIEW == 1 ]]; then mkdir -p "$PREV"; fi

filt='^\[mf_bpy\]|^\[turntable\]|Error|error:'

gen() {  # gen <script> <out.glb> <preview-prefix> [variant]
  local script="$1" outfile="$2" prev="$3" variant="${4:-}"
  local args=()
  if [[ -n "$variant" ]]; then args+=("$variant"); fi
  if [[ $PREVIEW == 1 ]]; then args+=("--preview" "$PREV/$prev"); fi
  args+=("$outfile")
  echo ">> $script ${variant:-} -> $outfile"
  "$BLENDER" --background --factory-startup --python "$HERE/$script" -- "${args[@]}" \
    2>&1 | grep -E "$filt" || true
}

# suspension bridge family
gen gen_bridge.py "$OUT/bridge_suspension.glb" bridge_suspension generic
gen gen_bridge.py "$OUT/bridge_brooklyn.glb"   bridge_brooklyn   brooklyn
# through-truss bridge (120-250m spans)
gen gen_truss.py  "$OUT/bridge_truss.glb"      bridge_truss
# metro consist
gen gen_train.py  "$OUT/train_metro.glb"       train_metro
# cloud puffs
gen gen_clouds.py "$OUT/cloud_puffs.glb"       cloud_puffs

echo "== assets =="
ls -la "$OUT"
if [[ $PREVIEW == 1 ]]; then echo "== previews =="; ls "$PREV"; fi
