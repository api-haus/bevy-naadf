#!/bin/bash
# Extract trajectory signals from a parity-gate log.
# Usage: trajectory-extract.sh <logfile>
set -u
log=$1
ratio=$(/bin/grep -oE 'cpu-gpu-parity\] same_bytes=[0-9]+ total_bytes=[0-9]+ ratio=[0-9.]+%' "$log" | /bin/head -1 | /bin/grep -oE 'ratio=[0-9.]+%')
gpu=$(/bin/grep -oE 'cpu-gpu-parity-interior-diff\] rank=0 chunk_idx=8449 .*gpu=\[[0-9,]+\]' "$log" | /bin/head -1 | /bin/grep -oE 'gpu=\[[0-9,]+\]')
ssim=$(/bin/grep -oE 'SSIM=[0-9.]+' "$log" | /bin/head -1)
passed=$(/bin/grep -cE '^  1 passed' "$log" 2>/dev/null)
if [ "${passed:-0}" -gt 0 ]; then
  pass="PASS"
else
  pass="FAIL"
fi
# Classify trajectory by SSIM if oracle ratio not available:
#  - ssim<0.75 → broken
#  - 0.75<=ssim<0.91 → partial
#  - ssim>=0.91 → lucky-pass
if [ -n "${ratio:-}" ]; then
  case "$ratio" in
    ratio=4.*) traj=broken ;;
    ratio=36.*|ratio=37.*) traj=partial ;;
    *) traj=other ;;
  esac
else
  ssimv=${ssim#SSIM=}
  if [ "$pass" = "PASS" ]; then
    traj=lucky-pass
  elif [ -n "$ssimv" ] && awk -v s="$ssimv" 'BEGIN{exit !(s+0 >= 0.75)}'; then
    traj=partial
  else
    traj=broken
  fi
fi
echo "traj=$traj pass=$pass ${ratio:-ratio=?} ${gpu:-gpu=?} ${ssim:-SSIM=?}"
