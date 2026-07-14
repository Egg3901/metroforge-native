#!/usr/bin/env bash
#
# Per-quality-tier in-city render smoke gate (native#102).
#
# native#102 shipped an in-city BLACK VOID on the Potato/Low tiers while the
# title screen and the High/Ultra tiers rendered fine — a *tier-specific*
# regression that a single-tier smoke could never catch. Worse, a plain
# full-screen unique-colour count at the 9km autostart overview is FOOLED on
# the low tiers: the HUD/goals panel/minimap chrome alone clears ~1200 unique
# colours even when the 3D world is a void, so the existing `>1000` in-city
# gate can pass on a genuinely unplayable low-tier build (exactly the
# "title-only gate missed an unplayable build" class from #95).
#
# This gate boots the game once per tier straight into NYC and drives the
# built-in verify harness (MF_VERIFY_DIR) to a daytime, hero-framed city shot
# (`default.png`) — the same frame on every tier, close enough that a rendered
# city scores many thousands of unique colours while a void scores ~UI-only.
# Each tier has its own floor set well above that UI-only floor and well below
# the measured software-rasteriser (lavapipe) reading, so a real void fails on
# exactly the tier that regressed without flaking on rasteriser noise.
#
# Usage: tier-smoke.sh <binary> <workdir> [display_base]
#   <binary>   path to the metroforge executable (packaged or target/*)
#   <workdir>  scratch dir for screenshots + logs (created)
#   Other runtime env vars are inherited.
#
# Exits non-zero (and prints a ::error::) if ANY tier renders below its floor.

set -uo pipefail

BIN="${1:?usage: tier-smoke.sh <binary> <workdir> [display_base]}"
WORK="${2:?usage: tier-smoke.sh <binary> <workdir> [display_base]}"
DISPLAY_BASE="${3:-90}"

# Per-tier unique-colour floor for the hero `default.png` frame. Measured on
# lavapipe (software Vulkan): potato~4900 low~3800 medium~5800 high~4700; a
# UI-only void frame is ~1200. Floors sit roughly halfway between.
TIERS=(potato low medium high)
declare -A FLOOR=( [potato]=2500 [low]=2000 [medium]=3000 [high]=2500 )

# Hard cap on a single tier's run. The verify harness runs the sim at 120x to
# reach daylight, frames the hero shot, then continues its own sequence and
# exits; we only need `default.png`, so we poll for it and stop early.
MAX_WAIT_S=180

mkdir -p "$WORK"
fail=0

for i in "${!TIERS[@]}"; do
  tier="${TIERS[$i]}"
  disp=$((DISPLAY_BASE + i))
  out="$WORK/$tier"
  mkdir -p "$out"
  shot="$out/default.png"
  log="$out/run.log"

  Xvfb ":$disp" -screen 0 1280x800x24 >/dev/null 2>&1 &
  xpid=$!
  sleep 3

  DISPLAY=":$disp" MF_QUALITY="$tier" MF_AUTOSTART=nyc MF_VERIFY_DIR="$out" \
    "$BIN" > "$log" 2>&1 &
  gpid=$!

  # Poll for the hero frame (verify harness writes it after it reaches
  # daytime + the dense centre), then give the async GPU->CPU readback a
  # moment to flush before we tear the process down.
  waited=0
  while [ "$waited" -lt "$MAX_WAIT_S" ]; do
    if [ -f "$shot" ]; then sleep 3; break; fi
    if ! kill -0 "$gpid" 2>/dev/null; then break; fi
    sleep 2
    waited=$((waited + 2))
  done

  kill "$gpid" 2>/dev/null || true
  wait "$gpid" 2>/dev/null || true
  kill "$xpid" 2>/dev/null || true

  floor="${FLOOR[$tier]}"
  if [ ! -f "$shot" ]; then
    echo "::error::tier-smoke FAILED [$tier]: no in-city frame was produced (harness never reached a rendered city)."
    echo "--- last 25 log lines [$tier] ---"; tail -25 "$log" 2>/dev/null || true
    fail=1
    continue
  fi

  colors=$(identify -format %k "$shot" 2>/dev/null || echo 0)
  echo "tier-smoke [$tier]: in-city unique colours = $colors (floor $floor)"
  if [ "$colors" -lt "$floor" ]; then
    echo "::error::tier-smoke FAILED [$tier]: $colors unique colours < $floor — in-city renders as a void on the $tier tier (native#102 regression)."
    echo "--- last 25 log lines [$tier] ---"; tail -25 "$log" 2>/dev/null || true
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  echo "::error::tier-smoke: one or more quality tiers rendered an in-city void."
  exit 1
fi
echo "tier-smoke: all ${#TIERS[@]} quality tiers rendered a city."
