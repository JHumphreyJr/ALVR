# Vision Pro lifecycle test scenarios

Reference for touchless-server / native-headset parity testing.  
Mark each: **PASS** | **FAIL** | **SKIP** + notes.

## A. Cold start (PC idle)

| ID | Scenario | Native headset expectation | What to watch |
|----|----------|---------------------------|---------------|
| A1 | PC running `alvr_server`, SteamVR down | N/A | Supervisor launches SteamVR; compositor healthy |
| A2 | Open ALVR on VP, connect | Link establishes quickly | Handshake time, first video / wireframes |
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
| D2 | Launch game from SteamVR (e.g. HL:Alyx menu) | Enters VR normally | Encoder, audio routing |
| D3 | **Exit VR** from Steam menu (stay in game) | Back to SteamVR shell; HMD still connected | **Known bug**: compositor crash, wireframes |
| D4 | Exit game entirely to desktop | SteamVR may stay running | vrserver/compositor state |
| D5 | Relaunch SteamVR from ALVR dashboard while VP connected | Should recover | Double vision risk |

## E. Network / room change

| ID | Scenario | Native expectation | What to watch |
|----|----------|------------------|---------------|
| E1 | Walk to another room (same WiFi) | May glitch; often recovers | UDP/TCP drop, reconnect time |
| E2 | Brief WiFi flap (airplane mode 5s) | Reconnect required | Supervisor recovery; backoff logs |

## F. Failure / recovery (supervisor)

| ID | Scenario | Native expectation | What to watch |
|----|----------|------------------|---------------|
| F1 | Kill `vrcompositor` while streaming | Auto recovery | Supervisor backoff, restart ≤2 then backoff |
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
2. D2 short play (30s in HL:Alyx or SteamVR Home)
3. B1 → B2 (take off / put on)
4. C1 → C2 (background / return)
5. D3 (Exit VR) — **critical**
6. C4 → C5 (kill app / reconnect)
7. D5 only if D3 failed badly
8. E1 if time permits

Log monitor output: `/tmp/alvr_lifecycle_test.log`
