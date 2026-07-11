# mf-wire v1

The wire protocol between `metroforge-native` (client) and `metroforge-sidecar`
(sim host), over a single WebSocket connection. `protocolVersion = 1` for
everything described in this document.

Two frame kinds share the socket:

- **Text frames** carry JSON control messages: handshake, init, commands, UI state,
  toasts. Low rate, except `ui` at 2 Hz.
- **Binary frames** carry hot-path typed payloads: per-tick vehicle/agent snapshots,
  scalar field grids, traffic overlays, static mask bytes. All binary data is
  **little-endian**.

This document is generated from, and was checked byte-for-byte against, two
independent implementations that must agree: the Rust decoder/encoder at
`crates/mf-protocol/src/binary.rs` (+ `types.rs`/`envelope.rs`) in this repo, and the
TypeScript encoder at `sidecar/wire.ts` in the sibling `metroforge` repo (currently
on its `feat/sim-sidecar` branch, not yet merged to `master`). As of this writing the
two agree exactly on every header field, every offset, and every field name. If a
future change makes them disagree, **the code wins**: specifically, whichever side
is actually deployed together wins over this document, and this document should be
updated rather than trusted blind. This document does not silently paper over any
currently-known discrepancy; there are none as of the last verification.

## 1. JSON envelope

Every text frame is exactly:

```json
{ "t": "<type>", "seq": 12, "p": { "...": "payload" } }
```

- `t`: message type string, always present.
- `seq`: a `u32`, present **only** on request/response-correlated messages (it
  carries the client-assigned `requestId` for `command` and `queryTrackCost`, and is
  echoed back on their corresponding `commandResult`/`trackCost` replies). Absent
  everywhere else.
- `p`: the payload object. Omitted entirely (not `null`) for payloadless messages
  (`requestSave`, `requestReplay`, `ping`, `shutdown`, `pong`, `bye`).

### 1.1 Client to sidecar

| type | seq | payload (`p`) | notes |
|---|---|---|---|
| `hello` | no | `{ clientProtocolVersion: 1 }` | first message after connecting |
| `init` | no | `{ seed: u64, difficulty: "easy"\|"normal"\|"hard", size?: "small"\|"medium"\|"large", presetKey?: string, rules?: ScenarioRules }` | starts a new game |
| `loadSave` | no | `{ json: string }` | loads a serialized save |
| `requestSave` | no | *(none)* | sidecar replies with `saved` |
| `setSpeed` | no | `{ speed: number }` | sim speed multiplier (0 = paused) |
| `command` | yes (= requestId) | `{ cmd: Command }` | one of the 11 `Command` variants, see below |
| `queryTrackCost` | yes (= requestId) | `{ mode: TransitMode, grade: TrackGrade, points: Vec2[] }` | cost preview, no state mutation |
| `requestReplay` | no | *(none)* | sidecar replies with `replay` |
| `ping` | no | *(none)* | liveness; sidecar replies `pong` |
| `shutdown` | no | *(none)* | sidecar stops its loop, replies `bye`, closes, exits 0 |

`Command` (internally tagged on `"kind"`, camelCase field names, 11 variants):
`buildStation{mode,pos}`, `buildTrack{mode,grade,fromStationId,toStationId,waypoints}`,
`createRoute{mode,stationIds}`,
`editRoute{routeId,headwaySeconds?,fare?,vehicleCount?,name?,color?}`,
`deleteRoute{routeId}`, `demolishStation{stationId}`, `demolishTrack{trackId}`,
`upgradeStation{stationId}`, `takeLoan{amount}`, `repayLoan{amount}`,
`renameStation{stationId,name}`.

### 1.2 Sidecar to client

| type | seq | payload (`p`) | notes |
|---|---|---|---|
| `hello` | no | `{ protocolVersion: 1, gameVersion: string, cityList: CityListEntry[], defaultWorldSize: number }` | sent immediately on connect, before any client message |

`CityListEntry` is `{ key, label }` plus optional additive fields the city-select
screen consumes when present: `country?`, `population?`, `buildingCount?`,
`sizeKm?`, and `mapPreview?` (`{ worldSize, res, water: number[], arterials:
number[][] }`). Older sidecars that only send `{key,label}` remain valid; the
native client fills gaps from its local catalog.
| `ready` | no | `{ staticCity: StaticCityJson }` | static city geometry, **minus** the three mask byte arrays (see `StaticMask` binary frame) |
| `demand` | no | `{ lines: {x1,y1,x2,y2,weight,share}[], maxWeight: number }` | droppable under backpressure |
| `ui` | no | the `UiState` struct directly as `p` | sent at 2 Hz; budget, approval, stations/tracks/routes, active events, etc. |
| `commandResult` | yes | `{ result: { ok: bool, error?: string, createdId?: i64 } }` | echoes the `command`'s `seq` |
| `trackCost` | yes | `{ cost: number }` | echoes the `queryTrackCost`'s `seq` |
| `saved` | no | `{ json: string }` | reply to `requestSave` |
| `replay` | no | the `ReplayPayload` struct directly as `p` | reply to `requestReplay`; always includes `stateHash` |
| `toast` | no | `{ message: string, tone: "info"\|"warn"\|"good" }` | |
| `pong` | no | *(none)* | reply to `ping` |
| `bye` | no | *(none)* | final message before the sidecar closes the socket, in response to `shutdown` |

`StaticCityJson` carries `fieldW`, `fieldH`, `cellSize`, `originX`, `originY`,
`worldSize`, `roadScale`, `maskRes?`, `hasWaterMask`, `hasParkMask`,
`hasBuildingMask`, `labels?`, and `roads` (each `{cls, points: flat x,y pairs}`).
It carries no raw mask bytes; those three optional masks arrive as separate binary
`StaticMask` frames (0 to 3 of them) immediately after `ready`.

`fields`, `traffic`, `frame`, and the three static masks are **binary**, not JSON:
covered in §2.

## 2. Binary frames

Every binary frame starts with the same two bytes: `byte 0 = msgType (u8)`,
`byte 1 = version (u8, currently always 1)`. All multi-byte fields are little-endian.
`f32`/`u32` arrays are **not** safe to cast in place from a raw buffer (a WebSocket
frame is not guaranteed 4-byte aligned): both implementations copy every array out
element-by-element (`chunks_exact(4)` in Rust, a `DataView`/typed-array blit in
TypeScript) rather than reinterpret-casting the backing buffer.

### msgType=1: FrameSnapshot (every 50 ms sim tick)

Header, 24 bytes:

| offset | type | field |
|---|---|---|
| 0 | u8 | msgType = 1 |
| 1 | u8 | version = 1 |
| 2 | u16 | reserved |
| 4 | u32 | tick |
| 8 | u32 | vehicleCount (`n`) |
| 12 | u32 | agentCount (`m`) |
| 16 | u32 | colorTableLen (`c`) |
| 20 | u32 | reserved |

Body, immediately following the header:

| offset | length | field |
|---|---|---|
| 24 | `4*c` bytes | `u32[c]` colorTable: packed `0x00RRGGBB` per route-color index |
| `24+4c` | `4*n*6` bytes | `f32[n*6]` vehicles, stride 6: `[id, x, y, heading, occupancy, routeColorIdx]` |
| `24+4c+24n` | `4*m*3` bytes | `f32[m*3]` agents, stride 3: `[x, y, phase]` (phase: 0 = walk, 1 = ride, 2 = wait) |

The native client ignores `colorTable`'s actual hex values by design (art direction:
the client keeps its own vivid color table indexed by `routeColorIdx`, so the same
index always means the same color everywhere; see `mf-render/src/palette.rs`). The
wire still carries the web palette's hex values because the sidecar reuses the
existing sim host code verbatim.

### msgType=2: Fields (init, then every 7 sim-days)

Header, 16 bytes:

| offset | type | field |
|---|---|---|
| 0 | u8 | msgType = 2 |
| 1 | u8 | version = 1 |
| 2 | u16 | reserved |
| 4 | u32 | fieldsVersion |
| 8 | u32 | cellCount (`N`) |
| 12 | u32 | reserved |

Body: four `f32[N]` arrays **then** two `u8[N]` arrays, in this exact order (this
differs from the TS `FieldsPayload` struct's field order: the f32 arrays are placed
first so every one of them starts 4-byte aligned from the frame start):

| offset | length | field |
|---|---|---|
| 16 | `4*N` | `f32[N]` terrain |
| `16+4N` | `4*N` | `f32[N]` population |
| `16+8N` | `4*N` | `f32[N]` jobs |
| `16+12N` | `4*N` | `f32[N]` landValue |
| `16+16N` | `N` | `u8[N]` water |
| `16+17N` | `N` | `u8[N]` parks |

`cellCount` is `fieldW * fieldH` from the most recent `StaticCityJson`; this frame
carries no grid dimensions of its own, so the client must already have `ready`.

### msgType=3: Traffic

Header, 32 bytes:

| offset | type | field |
|---|---|---|
| 0 | u8 | msgType = 3 |
| 1 | u8 | version = 1 |
| 2 | u16 | hotspotCount (`k`) |
| 4 | u32 | w |
| 8 | u32 | h |
| 12 | f32 | cellSize |
| 16 | f32 | originX |
| 20 | f32 | originY |
| 24 | u32 | valueCount (= `w*h`) |
| 28 | u32 | reserved |

Body:

| offset | length | field |
|---|---|---|
| 32 | `4*valueCount` | `f32[w*h]` values |
| `32+4*valueCount` | `12*k` | `(f32 x, f32 y, f32 severity)[k]` hotspots |

Out of v1 gameplay scope (no HUD surface consumes it yet) but decodable and covered
by `mf-protocol`'s fixture tests.

### msgType=4: StaticMask (0 to 3 frames, sent right after `ready`)

Header, 12 bytes:

| offset | type | field |
|---|---|---|
| 0 | u8 | msgType = 4 |
| 1 | u8 | version = 1 |
| 2 | u8 | which (0 = water, 1 = park, 2 = building) |
| 3 | u8 | reserved |
| 4 | u32 | res (`maskRes`) |
| 8 | u32 | reserved |

Body:

| offset | length | field |
|---|---|---|
| 12 | `res*res` | `u8[res*res]` mask, row-major |

Exactly one `StaticMask` frame is sent per mask flagged `true` in the preceding
`ready`'s `hasWaterMask`/`hasParkMask`/`hasBuildingMask` (procedural cities may send
zero). `mf-game`'s `Loading` state waits for `ready` plus every flagged mask, plus
the first `Fields` and first `ui`, before advancing to `InGame`.

## 3. Handshake, liveness, and shutdown

```
client                                   sidecar
  |                                          |
  |----------------- connect ------------->  |  (spawned as a child process,
  |                                          |   or already running)
  |  <---------------- hello ----------------|  {protocolVersion, gameVersion,
  |                                          |   cityList, defaultWorldSize}
  |----------------- hello ---------------->  |  {clientProtocolVersion}
  |     (client aborts if version mismatch)   |
  |                                          |
  |----------------- init ----------------->  |
  |  <---------------- ready ----------------|
  |  <------------ StaticMask x(0..3) -------|
  |  <---------------- fields ---------------|
  |  <---------------- ui -------------------|  (2 Hz, repeating)
  |  <---------------- frame ----------------|  (every 50 ms, repeating)
  |                                          |
  |----------------- ping ------------------>|  (every 5 s)
  |  <---------------- pong -----------------|
  |                                          |
  |  ... gameplay: command / commandResult, queryTrackCost / trackCost ...
  |                                          |
  |----------------- shutdown -------------->|
  |  <---------------- bye ------------------|
  |                                          |  (sidecar exits 0)
  X------------ socket closes --------------X
```

- The sidecar always sends its `hello` first, unprompted, immediately on connect.
- The client validates `protocolVersion === 1` and aborts the connection attempt on
  mismatch rather than trying to negotiate.
- **Liveness:** no inbound traffic (of any kind, including pongs) for **5 seconds**
  and the client declares the sim dead. Process exit is detected immediately via
  `Child::try_wait` and distinguished from websocket silence in
  `SidecarDeathReason`. `mf-net`'s reconnect policy then respawns the sidecar and
  reconnects with backoff starting at 500 ms, doubling up to a 4 s cap, for up to
  **3 attempts**. Mid-game, recovery re-handshakes, restores from the latest
  autosave (or re-inits the current city), and resumes `InGame` under a
  "Reconnecting to simulation" overlay — it does not bounce to MainMenu. After 3
  failures the client shows a diagnostics screen (log tail + copy button).
- The client pings every **2.5 seconds** (half the silence window) so an idle menu
  screen does not spuriously look dead.
- **Clean shutdown:** the client sends `shutdown`; the sidecar stops its tick loop,
  replies `bye`, closes the socket, and exits with code 0. `SidecarProcess::drop` is
  the backstop: if the child doesn't exit within a reasonable window, it is killed
  directly.

## 4. Backpressure

Before sending a **droppable** frame, the sidecar checks the WebSocket's outbound
buffered-byte count; if it exceeds 4 MiB, that specific frame is skipped rather than
queued. Droppable types: `frame`, `traffic`, `demand`. Every other message type is
**never** dropped: `hello`, `ready`, `staticMask`, `fields`, `ui`, `commandResult`,
`trackCost`, `saved`, `replay`, `toast`, `bye`. In practice this limit is rarely if
ever hit: the wire is estimated at roughly 1.8 MB/s at 3000 simultaneous vehicles,
far below what a local loopback WebSocket can sustain.

## 5. Versioning policy

`protocolVersion` (the JSON handshake field) and the binary frame `version` byte
(offset 1 of every binary frame) both currently read `1` and are kept in lockstep:
there is one protocol version number, not independently versioned JSON/binary
halves. Bump it when a change would break an older client or sidecar talking to a
newer counterpart: adding, removing, or reordering a binary frame's fields; changing
a JSON message's required fields (an optional, additive field does not require a
bump); changing a message's semantics without changing its shape. A client that
receives a `hello` with a `protocolVersion` it doesn't recognize aborts rather than
attempting best-effort compatibility: there is no negotiation between versions in
v1. Bumping the version means bumping it in both `mf_protocol::PROTOCOL_VERSION`
(Rust) and `wire.ts`'s `PROTOCOL_VERSION` (TypeScript) together; a mismatch between
those two constants is exactly the failure mode the handshake exists to catch early.
