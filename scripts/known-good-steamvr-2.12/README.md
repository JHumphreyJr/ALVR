# Known-good restore: ALVR + SteamVR 2.12.14 + Apple Vision Pro

This documents the **best-performing stable stack** observed on the Tensorbook
(Ubuntu 24, RTX 3080) with the **App Store ALVR client** on Apple Vision Pro.

Use this when newer SteamVR / OpenVR experiments regress (error 303, wireframes,
double vision, compositor crashes).

## Stack snapshot

| Component | Version / setting |
|-----------|-------------------|
| **SteamVR** | **2.12.14** (Steam beta branch `steamvr_2_12`, build `22542555`) |
| **ALVR server** | **v20.14.1 stock** — OpenVR SDK **1.16.8**, `IVRDisplayComponent_002` |
| **Git commit** | `a9f6542f` (tag `v20.14.1`) |
| **OpenVR submodule** | `4c85abc` (v1.16.8) |
| **VP client** | App Store ALVR v20, **RealityKit renderer** (not Metal) |
| **Protocol** | v20 (must match server major version) |

### Verified launcher install (no rebuild required)

```
~/.local/share/ALVR-Launcher/installations/v20.14.1/alvr_streamer_linux/
```

Driver: `lib64/alvr/bin/linux64/driver_alvr_server.so` (~87 MB, `IVRDisplayComponent_002`)

### Session settings (see `session.json` in this folder)

| Setting | Value |
|---------|-------|
| Stream protocol | **TCP** |
| Codec | **HEVC** |
| Bitrate | **150 Mbps** constant |
| Transcoding resolution | **3072 × 1536** per eye |
| Emulated headset resolution | **2144 × 1072** (`height.set` **must be true**) |
| Preferred FPS | **72** |
| Microphone | **disabled** |
| Recentering | **disabled** (position + rotation) |
| Foveated encoding | enabled (stock default) |
| 10-bit encoder | **off** |

### Network

- PC and Vision Pro on the **same subnet** (e.g. `192.168.1.x`)
- VP client ID: `6515.client.alvr` (trusted in session)
- Vision Pro log pipe: `nc <pc-ip> 9920` → `scripts/visionos_log_receiver.py`

---

## Quick restore (use existing stock install)

### 1. Pin SteamVR to 2.12

In the **Steam client** (not in-game):

1. Library → **SteamVR** → Properties → **Betas**
2. Select beta branch: **`steamvr_2_12`**
3. Let Steam download/downgrade (~400 MB)
4. Verify in logs after launch: `vrserver 2.12.14` / build `22542555`

> **Do not** stay on `steamvr_2_16` or default — 2.16.7 triggers error 303 on this setup.

### 2. Restore session config

```bash
cp scripts/known-good-steamvr-2.12/session.json ~/.config/alvr/session.json
```

Or run the helper script (session + SteamVR settings hints):

```bash
./scripts/known-good-steamvr-2.12/restore.sh
```

### 3. Fix vrcompositor wrapper (required after every SteamVR version change)

ALVR replaces `vrcompositor` with a Vulkan-layer wrapper. After a SteamVR
downgrade/upgrade, a **stale `vrcompositor.real`** from another version breaks
the wrapper.

```bash
STEAMVR_BIN="$HOME/.local/share/Steam/steamapps/common/SteamVR/bin/linux64"
WRAPPER="$HOME/.local/share/ALVR-Launcher/installations/v20.14.1/alvr_streamer_linux/libexec/alvr/vrcompositor-wrapper"

killall vrserver vrmonitor vrcompositor 2>/dev/null

# Remove symlink; drop stale .real from wrong SteamVR version
if [ -L "$STEAMVR_BIN/vrcompositor" ]; then
  rm -f "$STEAMVR_BIN/vrcompositor"
elif [ -f "$STEAMVR_BIN/vrcompositor" ]; then
  rm -f "$STEAMVR_BIN/vrcompositor.real"
  mv "$STEAMVR_BIN/vrcompositor" "$STEAMVR_BIN/vrcompositor.real"
fi

ln -sf "$WRAPPER" "$STEAMVR_BIN/vrcompositor"
ls -la "$STEAMVR_BIN/vrcompositor"*
```

### 4. SteamVR settings (`~/.local/share/Steam/config/steamvr.vrsettings`)

Ensure:

```json
"steamvr": {
  "enableHomeApp": false,
  "blocked_by_safe_mode": false   // under driver_alvr_server if present
}
```

SteamVR Home crashes on Linux; keep it disabled.

### 5. Launch

1. Start **ALVR Launcher** → run **v20.14.1** streamer (stock, **not** `appstore-stable` or `avp-stable`)
2. Open **ALVR on Vision Pro** (RealityKit renderer)
3. Connect from dashboard

**Healthy signs:** `VR_Init successful`, no persistent 303, warp mesh ~99.98%,
`streaming started` in VP logs.

**Known quirk:** first connect may show wireframes; second attempt often loads
SteamVR void/grid environment. SteamVR Home stays disabled.

---

## Rebuild from source (same binaries as stock v20.14.1)

Use this if the launcher install is missing or you need a clean compile.

### Prerequisites (Tensorbook, Jul 2026)

| Tool | Version used |
|------|----------------|
| OS | Ubuntu 24.04 |
| GPU | NVIDIA RTX 3080 |
| CUDA | 13.3 (`nvcc` in PATH, `/usr/local/cuda/lib64`) |
| Rust | 1.96.1 (`rustup` stable) |
| Build deps | `build-essential`, `cmake`, `nasm`, `pkg-config`, `libva-dev`, `libdrm-dev`, `vulkan-tools`, etc. |

### Build steps

```bash
cd /home/joe/Development/gaming/ALVR

# Clean tree at stock v20.14.1 — do NOT use patched branches
git fetch --tags
git checkout v20.14.1
git submodule update --init openvr   # must resolve to 4c85abc (OpenVR 1.16.8)

# Verify OpenVR version
git -C openvr describe --tags   # expect v1.16.8

# Dependencies (x264, FFmpeg w/ NVENC) — one-time, ~30+ min
cargo xtask prepare-deps --platform linux

# Build release streamer
cargo xtask build-streamer --release

# Output directory
ls build/alvr_streamer_linux/
```

### Install into ALVR Launcher

Copy `build/alvr_streamer_linux/` into the launcher installations folder, or
use **ALVR Launcher → Install from archive** if you package it:

```bash
cargo xtask package-streamer --platform linux
# produces build/alvr_streamer_linux.tar.gz
```

Target path:

```
~/.local/share/ALVR-Launcher/installations/v20.14.1/alvr_streamer_linux/
```

After install, repeat **vrcompositor wrapper** steps above using the new
install's `libexec/alvr/vrcompositor-wrapper`.

### Verify driver interface

```bash
strings ~/.local/share/ALVR-Launcher/installations/v20.14.1/alvr_streamer_linux/lib64/alvr/bin/linux64/driver_alvr_server.so \
  | grep IVRDisplayComponent
# Must show: IVRDisplayComponent_002
# Must NOT show: IVRDisplayComponent_003
```

---

## What NOT to use for this restore point

| Artifact | Why |
|----------|-----|
| Branch `v20-avp-stable` | OpenVR 2.15.6, NVENC fork — targets SteamVR 2.15+, not 2.12 |
| Branch `v20.14.1-appstore-stable` (patched) | Current experimental work; OpenVR 2.15.6 uncommitted patches |
| Install `v20.14.1-appstore-stable` | `IVRDisplayComponent_003` — incompatible with SteamVR 2.12 |
| Install `v20.14.1-avp-stable` | Fork build, OpenVR 2.15.6 |
| SteamVR 2.16.7 | Error 303 / compositor IPC failures on this hardware |

---

## Touchless PC (normal use)

**Goal:** launch ALVR once at login, then operate entirely from the Vision Pro.

| Use | Binary |
|-----|--------|
| **Normal desktop** | **`alvr_dashboard`** via ALVR Launcher — GUI + SteamVR supervisor |
| **Optional boot service** | `alvr_server` (no window) — same supervisor; see `alvr-server.service` |

Enable in session: `extra.steamvr_launcher.open_close_steamvr_with_dashboard: true`.

The supervisor (in dashboard since commit `cf88690f`):

- Auto-launches SteamVR when the dashboard starts
- Does **not** require `vrcompositor` while idle (no headset connected)
- Debounces recovery (3 polls) and ignores healthy warp-mesh log lines (`shrink wrap saved 0.00%`)
- Restarts SteamVR on real compositor failure with exponential backoff (5 s → 300 s cap)

**Do not run dashboard and `alvr_server` at the same time** — two supervisors will fight.

---

## Monitoring

```bash
# Vision Pro logs (run on PC)
python3 scripts/visionos_log_receiver.py   # listens on 0.0.0.0:9920

# VP pipes logs:
nc 192.168.1.170 9920

# Combined monitor
tail -f /tmp/alvr_monitor.log
```

SteamVR logs: `~/.local/share/Steam/logs/{vrserver,vrcompositor,vrmonitor}.txt`

---

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| Error **303** | Pin SteamVR to `steamvr_2_12`; fix vrcompositor wrapper |
| **Wireframes** on VP | Compositor crash or no video; check warp mesh 0.00% in vrcompositor.txt |
| **Double vision** | Distortion mesh stale; force-quit ALVR + Restart SteamVR in dashboard |
| **Viewport shifted right / visible window rect** | After Exit VR relaunch; stale ViewsConfig — force-quit ALVR, Restart SteamVR, reconnect (Phase 4 fix pending) |
| **Slow Enter VR** / dashboard banner | Server core starts with driver on connect; wait for SteamVR: Connected (Phase 2 daemon) |
| **Blue lines after headset-off** | Session dropped; force-quit ALVR + Restart SteamVR |
| VP can't find PC | Same WiFi subnet; check `ip addr` on both devices |
| Safe mode / driver blocked | Set `driver_alvr_server.blocked_by_safe_mode: false` in steamvr.vrsettings |
| `emulated height` wrong | Must be `set: true, content: 1072` — see `session.json` here |
| Supervisor killed SteamVR mid-session | Fixed in `cf88690f`; rebuild and redeploy dashboard |

---

## File manifest (this restore kit)

```
scripts/known-good-steamvr-2.12/
├── README.md                      # this file
├── ARCHITECTURE.md                # component diagram, driver load path
├── REQUIREMENTS-headless-server.md # touchless PC requirements + phases
├── VP-LIFECYCLE-SCENARIOS.md      # test matrix + 2026-07-09 results
├── session.json                   # reference session (sync openvr_config)
├── restore.sh                     # copy session + SteamVR hints
├── build-from-source.sh
└── alvr-server.service            # optional systemd unit (headless binary)
```

### Recent server commits (branch `v20.14.1-appstore-stable`)

| Commit | Summary |
|--------|---------|
| `8c94dd18` | First-connect stability (defer LensDistortionChanged, presync) |
| `07a733a6` | Linux audio routing to ALVR virtual sink |
| `cf88690f` | Dashboard SteamVR supervisor + optional `alvr_server` binary |

Documented: 2026-07-09
