# Vision Pro lifecycle test scenarios

Reference for touchless-server / native-headset parity testing.  
Mark each: **PASS** | **FAIL** | **PARTIAL** | **SKIP** + notes.

## Latest session — 2026-07-09 (Tensorbook, SteamVR 2.12.14, dashboard + supervisor)

**Entry point:** `alvr_dashboard` (not headless `alvr_server`).  
**Session:** high-bandwidth UDP profile in `~/.config/alvr/session.json`.  
**Log monitor:** `/tmp/alvr_lifecycle_test.log`

| ID | Result | Notes |
|----|--------|-------|
| A1 | **PASS** | Dashboard autostarted SteamVR via supervisor |
| A2 | **PARTIAL** | Connect worked; Enter VR slow until dashboard banner cleared (server core in driver only starts after client connects) |
| A3 | **PASS** | Stable image in SteamVR Home / grid |
| B1 | **PASS** | Headset off mid-game → game paused (PC preview) |
| B2 | **PARTIAL** | First attempt: blue lines + black PC preview (TCP dropped). Second attempt same session: **PASS** — resume mid-game worked |
| B3 | SKIP | Not tested |
| C1–C2 | SKIP | Not tested this session |
| C4–C5 | **PASS** | Force-quit + reconnect after B2 failure restored clean image |
| D1 | **PASS** | Grid shell stable |
| D2 | **PASS** | Pistol Whip playable |
| D3 | **PARTIAL** | Exit game → grid → Steam menu **Exit VR** on PC → SteamVR closed, VP wireframes (expected). Relaunch ALVR from visionOS home → dashboard **auto-launched SteamVR** (**PASS**). Enter VR → **FAIL**: viewport shifted right, rectangular window visible, controllers misaligned |
| D4 | PARTIAL | Exit VR closed SteamVR entirely (may be game/shell dependent) |
| D5 | SKIP | Covered implicitly by D3 relaunch path |
| F1–F3 | SKIP | Supervisor false-positive kill fixed; no manual kill tests |
| G1 | PASS | Audio in game (prior sessions) |

### Open bugs from this session (priority)

1. **Exit VR → relaunch:** stale or wrong `ViewsConfig` / compositor state → viewport offset, visible stream rectangle (Phase 4 + client K4).
2. **Cold / warm connect latency:** dashboard banner until `:8082` live (Phase 2 daemon).
3. **B2 intermittent:** headset-off may drop TCP instead of standby-resume (server + client proximity).

### Workarounds until fixed

| Symptom | Workaround |
|---------|------------|
| Blue lines / black preview after headset-off | Force-quit ALVR on VP; **Restart SteamVR** in dashboard; reconnect |
| Viewport shifted / rectangular window after Exit VR relaunch | Force-quit ALVR on VP; Restart SteamVR; reconnect (full handshake) |
| Slow Enter VR button | Wait for dashboard “SteamVR: Connected” (green); banner clears when driver is up |

---

## A. Cold start (PC idle)

| ID | Scenario | Native headset expectation | What to watch |
|----|----------|---------------------------|---------------|
| A1 | PC running **dashboard**, SteamVR down | N/A | Supervisor launches SteamVR; compositor healthy when streaming |
| A2 | Open ALVR on VP, connect | Link establishes quickly | Handshake time, dashboard banner, Enter VR delay |
| A3 | First video appears | Immediate usable image | IDR, warp mesh %, `deferring LensDistortionChanged` |

## B. Wear / remove (no app kill)

| ID | Scenario | Native expectation | What to watch |
|----|----------|------------------|---------------|
| B1 | Remove headset (on desk), app foreground | Standby, session persists | TCP stays up; encode may continue |
| B2 | Put headset back on | Resume without reconnect | Video resumes without full handshake |
| B3 | Remove headset 2+ minutes | May standby / dim | Server still Streaming? |

## C. visionOS app lifecycle

| ID | Scenario | Native expectation | What to watch |
|----|----------|------------------|---------------|
| C1 | Swipe away / background ALVR | Session often pauses | Disconnect vs idle; server state |
| C2 | Return to ALVR from app switcher | Resume or fast reconnect | Handshake again? video delay |
| C3 | visionOS notification → other app → back | Same as C2 | Client logs, server Streaming state |
| C4 | Force-quit ALVR app | Clean disconnect | Server → Disconnected; SteamVR stays? |
| C5 | Reopen ALVR after force-quit | Full reconnect works | Connect latency; first-frame time |

## D. SteamVR / in-VR actions

| ID | Scenario | Native expectation | What to watch |
|----|----------|------------------|---------------|
| D1 | SteamVR Home / void visible after connect | Lobby shell | Home vs grid; compositor stable |
| D2 | Launch game from SteamVR (e.g. Pistol Whip) | Enters VR normally | Encoder, audio routing |
| D3 | **Exit VR** from Steam menu, then relaunch ALVR | Back to shell; HMD reconnects with correct view | Wireframes OK; **viewport shift / window rect on relaunch** |
| D4 | Exit game entirely to desktop | SteamVR may stay running | vrserver/compositor state |
| D5 | Relaunch SteamVR from ALVR dashboard while VP connected | Should recover | Double vision / offset risk |

## E. Network / room change

| ID | Scenario | Native expectation | What to watch |
|----|----------|------------------|---------------|
| E1 | Walk to another room (same WiFi) | May glitch; often recovers | UDP/TCP drop, reconnect time |
| E2 | Brief WiFi flap (airplane mode 5s) | Reconnect required | Supervisor recovery; backoff logs |

## F. Failure / recovery (supervisor)

| ID | Scenario | Native expectation | What to watch |
|----|----------|------------------|---------------|
| F1 | Kill `vrcompositor` while streaming | Auto recovery | Supervisor backoff; no false positive on `shrink wrap saved 0.00%` |
| F2 | Kill `vrserver` while streaming | Auto recovery | Driver reload, client reconnect |
| F3 | SteamVR safe mode (driver blocked) | Unblock + relaunch | `blocked_by_safe_mode` cleared |

## G. Audio

| ID | Scenario | What to watch |
|----|----------|---------------|
| G1 | In-game audio after connect | `ALVR Audio` default sink; headset output |
| G2 | After Exit VR / reconnect | Audio still routed |

---

## Suggested test order (one session ~20–30 min)

1. A1 → A3 (cold connect)
2. D2 short play (30s in game or SteamVR Home)
3. B1 → B2 (take off / put on)
4. C1 → C2 (background / return)
5. D3 (Exit VR + relaunch ALVR) — **critical**
6. C4 → C5 (kill app / reconnect)
7. D5 only if D3 failed badly
8. E1 if time permits

Log monitor:

```bash
: > /tmp/alvr_lifecycle_test.log
tail -n 0 -F /tmp/alvr_dashboard.log \
  ~/.local/share/Steam/logs/vrserver.txt \
  ~/.local/share/Steam/logs/vrcompositor.txt \
  | grep -E --line-buffered 'supervisor|recovering|Warp mesh|Streaming|standby|LensDistortion' \
  >> /tmp/alvr_lifecycle_test.log &
```
