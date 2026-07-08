#!/usr/bin/env bash
# Restore known-good ALVR session for SteamVR 2.12.14 + stock v20.14.1.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SESSION_SRC="$SCRIPT_DIR/session.json"
SESSION_DST="${HOME}/.config/alvr/session.json"
STEAMVR_SETTINGS="${HOME}/.local/share/Steam/config/steamvr.vrsettings"

echo "=== ALVR known-good restore (SteamVR 2.12.14) ==="

if [[ ! -f "$SESSION_SRC" ]]; then
  echo "ERROR: missing $SESSION_SRC" >&2
  exit 1
fi

mkdir -p "$(dirname "$SESSION_DST")"
if [[ -f "$SESSION_DST" ]]; then
  cp "$SESSION_DST" "${SESSION_DST}.bak-$(date +%Y%m%d-%H%M%S)"
  echo "Backed up existing session.json"
fi
cp "$SESSION_SRC" "$SESSION_DST"
echo "Installed session → $SESSION_DST"

# Best-effort SteamVR settings patch (disable Home, note version)
if [[ -f "$STEAMVR_SETTINGS" ]] && command -v python3 >/dev/null; then
  python3 - <<'PY'
import json, os
p = os.path.expanduser("~/.local/share/Steam/config/steamvr.vrsettings")
d = json.load(open(p))
d.setdefault("steamvr", {})["enableHomeApp"] = False
drivers = d.setdefault("driver_alvr_server", {})
if isinstance(drivers, dict):
    drivers["blocked_by_safe_mode"] = False
json.dump(d, open(p, "w"), indent=2)
print("Updated steamvr.vrsettings (enableHomeApp=false, unblocked driver)")
PY
fi

cat <<'EOF'

--- Manual steps still required ---

1. Pin SteamVR beta: steamvr_2_12  (Steam → Library → SteamVR → Properties → Betas)
2. Fix vrcompositor wrapper — see README.md section 3
3. Launch stock v20.14.1 from ALVR Launcher (NOT appstore-stable / avp-stable)
4. Kill stale SteamVR before relaunch:
     killall vrserver vrmonitor vrcompositor

Verify after launch:
  grep "vrserver 2.12" ~/.local/share/Steam/logs/vrserver.txt | tail -1

EOF
