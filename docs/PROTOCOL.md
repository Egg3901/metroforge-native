# mf-wire v1

Wire protocol between `metroforge-native` (client) and `metroforge-sidecar`
(sim host) over one WebSocket. `PROTOCOL_VERSION = 1`
(`crates/mf-protocol/src/lib.rs`).

Two frame kinds share the socket:

- **Text frames** — JSON control messages (`crates/mf-protocol/src/envelope.rs`,
  `types.rs`).
- **Binary frames** — little-endian hot-path payloads
  (`crates/mf-protocol/src/binary.rs`). Arrays are copied via `chunks_exact(4)` /
  byte copies; the WS buffer is not assumed 4-byte aligned.

Source of truth for layouts and field optionality is the Rust codec in this
repo. If a deployed sidecar disagrees, the code that is actually paired wins
and this document should be updated.

---

## 1. JSON envelope

Every text frame deserializes as (`envelope.rs`):

```json
{ "t": "<type>", "seq": 12, "p": { "...": "payload" } }
```

| Field | Optional | Notes |
|---|---|---|
| `t` | required | Message type string |
| `seq` | optional | Present only on request/response-correlated messages; carries the client `requestId` |
| `p` | optional | Omitted entirely (not `null`) for payloadless messages |

Serde: `#[serde(default, skip_serializing_if = "Option::is_none")]` on `seq` and `p`.

### 1.1 Client → sidecar (`ToSim`)

| `t` | `seq` | `p` | Rust | Notes |
|---|---|---|---|---|
| `hello` | no | `{ clientProtocolVersion: u32 }` | `ClientHelloPayload` | First client message after WS connect |
| `init` | no | see below | `InitPayload` | Starts a new game |
| `loadSave` | no | `{ json: string }` | `LoadSavePayload` | Loads a serialized save |
| `requestSave` | no | *(none)* | — | Sidecar replies `saved` |
| `setSpeed` | no | `{ speed: f64 }` | `SetSpeedPayload` | `0` = paused |
| `command` | yes (= requestId) | `{ cmd: Command }` | `CommandPayload` | See Command table |
| `queryTrackCost` | yes (= requestId) | `{ mode, grade, points }` | `QueryTrackCostPayload` | Cost preview; no mutation |
| `strataProbe` | yes (= requestId) | `{ x, y }` | `StrataProbePayload` | Subsurface column probe (v0.8); no mutation |
| `requestReplay` | no | *(none)* | — | Sidecar replies `replay` |
| `ping` | no | *(none)* | — | Liveness; sidecar replies `pong` |
| `shutdown` | no | *(none)* | — | Sidecar replies `bye`, closes |

#### `InitPayload` (`camelCase`)

| Field | Type | Optional | Notes |
|---|---|---|---|
| `seed` | `u64` | required | |
| `difficulty` | `"easy"\|"normal"\|"hard"` | required | |
| `size` | `"small"\|"medium"\|"large"` | optional | |
| `presetKey` | `string` | optional | City preset key |
| `rules` | `ScenarioRules` | optional | See §1.3 |

#### `QueryTrackCostPayload`

| Field | Type | Optional |
|---|---|---|
| `mode` | `TransitMode` | required |
| `grade` | `TrackGrade` | required |
| `points` | `Vec2[]` | required |

`TransitMode`: `"bus"\|"tram"\|"metro"\|"rail"`.  
`TrackGrade`: `"surface"\|"elevated"\|"tunnel"`.  
`Vec2`: `{ x: f64, y: f64 }`.

#### `Command` (internally tagged on `"kind"`, camelCase fields)

| `kind` | Fields | Optionality |
|---|---|---|
| `buildStation` | `mode`, `pos` | required |
| `buildTrack` | `mode`, `grade`, `fromStationId`, `toStationId`, `waypoints` | required |
| `createRoute` | `mode`, `stationIds` | required |
| `editRoute` | `routeId`; `headwaySeconds?`, `fare?`, `vehicleCount?`, `name?`, `color?` | optionals use `default` + `skip_serializing_if` |
| `deleteRoute` | `routeId` | required |
| `demolishStation` | `stationId` | required |
| `demolishTrack` | `trackId` | required |
| `upgradeStation` | `stationId` | required |
| `takeLoan` | `amount` | required |
| `repayLoan` | `amount` | required |
| `renameStation` | `stationId`, `name` | required |

### 1.2 Sidecar → client (`FromSimJson`)

| `t` | `seq` | `p` | Notes |
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
| `trackCost` | yes | `{ cost: number, breakdown?: TrackCostBreakdown }` | echoes the `queryTrackCost`'s `seq` |
| `strataProbe` | yes | `StrataProbeResultPayload` | echoes the `strataProbe`'s `seq` |
| `saved` | no | `{ json: string }` | reply to `requestSave` |
| `replay` | no | the `ReplayPayload` struct directly as `p` | reply to `requestReplay`; always includes `stateHash` |
| `toast` | no | `{ message: string, tone: "info"\|"warn"\|"good" }` | |
| `pong` | no | *(none)* | reply to `ping` |
| `bye` | no | *(none)* | final message before the sidecar closes the socket, in response to `shutdown` |

#### `HelloInfo` (`camelCase`)

| Field | Type | Optional |
|---|---|---|
| `protocolVersion` | `u32` | required |
| `gameVersion` | `string` | required |
| `cityList` | `{ key, label }[]` | required |
| `defaultWorldSize` | `f64` | required |

#### `StaticCityJson` (`camelCase`)

| Field | Type | Optional / default | Notes |
|---|---|---|---|
| `fieldW` | `u32` | required | |
| `fieldH` | `u32` | required | |
| `cellSize` | `f64` | required | |
| `originX` | `f64` | required | |
| `originY` | `f64` | required | |
| `worldSize` | `f64` | required | |
| `roadScale` | `f64` | required | |
| `maskRes` | `u32` | optional | |
| `hasWaterMask` | `bool` | default `false` | Expect msgType=4 `which=0` |
| `hasParkMask` | `bool` | default `false` | Expect msgType=4 `which=1` |
| `hasBuildingMask` | `bool` | default `false` | Expect msgType=4 `which=2` |
| `labels` | `MapLabel[]` | optional | |
| `roads` | `{ cls: string, points: f64[] }[]` | required | Flat x,y pairs per segment |

Each `roads[]` entry is a `RoadDto` (`types.rs`):

| Field | Type | Optional / default | Notes |
|---|---|---|---|
| `cls` | `string` | required | `arterial` / `collector` / `local`, … |
| `points` | `f64[]` | required | Flat x,y pairs |
| `gradeLevel` | `i32` | default `0` | OSM layer/bridge/tunnel grade separation |
| `isBridge` | `bool` | default `false` | Bridge deck segment |
| `isTunnel` | `bool` | default `false` | Tunnel segment |

No raw mask bytes in JSON; those arrive as binary `StaticMask` (0–3 frames)
immediately after `ready`.

#### `UiState` (`camelCase`) — sent as `p` of `t:"ui"`

| Field | Type | Optional / default |
|---|---|---|
| `tick` | `u64` | required |
| `insights` | `string[]` | required |
| `day` | `u32` | required |
| `speed` | `f64` | required |
| `cash` | `f64` | required |
| `loanBalance` | `f64` | required |
| `lastDay` | `DayLedger` | required |
| `netHistory` | `f64[]` | required |
| `population` | `f64` | required |
| `approval` | `f64` | required |
| `transitShare` | `f64` | required |
| `coverage` | `f64` | required |
| `dailyTransitTrips` | `f64` | required |
| `unlockedModes` | `TransitMode[]` | required |
| `stations` | `UiStation[]` | required |
| `tracks` | `UiTrack[]` | required |
| `routes` | `UiRoute[]` | required |
| `activeEvents` | `ActiveEventDto[]` | required |
| `fieldsVersion` | `u32` | required |
| `bankrupt` | `bool` | required |
| `failed` | `"bankrupt"\|"approval"\|"time"\|null` | default `null` |
| `maxDay` | `u32` | optional |
| `eraLabel` | `string` | optional |
| `commandCount` | `u32` | required |
| `hourOfDay` | `f64` | default omit / `null` (sim-depth) |
| `demandFactor` | `f64` | default omit (sim-depth) |
| `fareboxRecovery` | `f64` | default omit (sim-depth) |
| `lifetime` | `f64` | default omit (sim-depth) |
| `districts` | `UiDistrict[]` | default `[]` (sim-depth) |
| `overcrowdedRoutes` | `u32` | default omit (sim-depth) |
| `weatherState` | `"clear"\|"overcast"\|"rain"\|"fog"\|"snow"\|"storm"` | default omit (v0.7) |
| `weatherIntensity` | `f64` | default omit (v0.7) — precip strength / heat for clear |
| `weatherSeason` | `"winter"\|"spring"\|"summer"\|"autumn"` | default omit (v0.7) |
| `weatherEvent` | `"blizzard"\|"heatwave"` | default omit (v0.7) |

`DayLedger`: `{ fares, subsidy, operations, maintenance, interest }` (all `f64`).

`UiStation`: `id`, `name`, `x`, `y`, `mode`, `level`, `ridership`, `alightings`.

`UiTrack`: `id`, `mode`, `grade` (**string**, not `TrackGrade` enum), `points`,
`fromStationId`, `toStationId`.

`UiRoute`: required fields plus optional sim-depth `liveCrowding`,
`operatingCost`, `farebox` (each `#[serde(default)]`).

`display_hour()` (`types.rs`): prefers finite `hourOfDay`; else
`(tick % 1200) / 1200 * 24` (`TICKS_PER_DAY = 1200`).

#### `DemandPayload`

| Field | Type |
|---|---|
| `lines` | `{ x1,y1,x2,y2,weight,share }[]` |
| `maxWeight` | `f64` |

#### `TrackCostPayload` / `TrackCostBreakdown` (v0.8, additive)

`trackCost` replies wrap:

| Field | Type | Optional / default |
|---|---|---|
| `cost` | `f64` | required |
| `breakdown` | `TrackCostBreakdown` | default omit — old sidecars send cost only |

`TrackCostBreakdown` (`camelCase`, all fields `#[serde(default)]`):

| Field | Type | Notes |
|---|---|---|
| `surface` | `f64` | Reference surface alignment cost |
| `elevated` | `f64` | Reference elevated alignment cost |
| `cutCover` | `f64` | Cut-and-cover component along the line |
| `bored` | `f64` | Bored component along the line |
| `strata` | `string` | Dominant strata crossed, e.g. `"fill/clay/rock"` |
| `belowWaterTable` | `bool` | Any segment below the water table |

#### `StrataProbePayload` / `StrataProbeResultPayload` (v0.8)

Client `strataProbe` sends `{ x, y }` (world meters). Reply `p` is:

| Field | Type | Notes |
|---|---|---|
| `bands` | `{ kind, top, bottom }[]` | `kind`: `fill` / `clay` / `rock` / `bedrock`; depths in meters below surface |
| `waterTable` | `f64` | Depth (m) to water table |
| `rockHardness` | `f64` | Competent-rock hardness `0..1` |
| `surfaceElevation` | `f64` | Surface elevation (m above sea level) |

#### `CommandResult`

| Field | Type | Optional |
|---|---|---|
| `ok` | `bool` | required |
| `error` | `string` | optional |
| `createdId` | `i64` | optional |

#### `ReplayPayload` (`camelCase`)

| Field | Type | Optional |
|---|---|---|
| `seed` | `u64` | required |
| `difficulty` | `Difficulty` | required |
| `presetKey` | `string` | optional |
| `size` | `CitySize` | optional |
| `rules` | `ScenarioRules` | optional |
| `commandLog` | `{ tick, cmd }[]` | required |
| `finalTick` | `u64` | required |
| `stateHash` | `i64` | required |
| `scoreHint` | `f64` | required |

### 1.3 `ScenarioRules` (`camelCase`)

| Field | Type | Optional |
|---|---|---|
| `scenarioId` | `string` | optional |
| `startingModes` | `TransitMode[]` | required |
| `lockModes` | `bool` | optional |
| `maxDay` | `u32` | optional |
| `approvalFloor` | `f64` | optional |
| `startingCash` | `f64` | optional |
| `dailySubsidy` | `f64` | optional |
| `eraLabel` | `string` | optional |

---

## 2. Binary frames

Common prefix: byte 0 = `msgType` (`u8`), byte 1 = `version` (`u8`).
All multi-byte scalars are little-endian.

`decode_binary` dispatches on byte 0 (`binary.rs`).

### msgType=1: `FrameSnapshot` (every 50 ms sim tick)

Wire version: **1 only**. Header = 24 bytes.

| Offset | Type | Field |
|---|---|---|
| 0 | `u8` | msgType = 1 |
| 1 | `u8` | version = 1 |
| 2 | `u16` | reserved |
| 4 | `u32` | tick |
| 8 | `u32` | vehicleCount (`n`) |
| 12 | `u32` | agentCount (`m`) |
| 16 | `u32` | colorTableLen (`c`) |
| 20 | `u32` | reserved |

| Offset | Length | Field |
|---|---|---|
| 24 | `4*c` | `u32[c]` colorTable (`0x00RRGGBB`) |
| `24+4c` | `4*n*6` | `f32[n*6]` vehicles: `[id, x, y, heading, occupancy, routeColorIdx]` |
| `24+4c+24n` | `4*m*3` | `f32[m*3]` agents: `[x, y, phase]` (0 walk, 1 ride, 2 wait) |

Client paint: `mf-render/src/vehicles.rs` **ignores** colorTable hex values and
indexes `palette::vivid_route_color` by `routeColorIdx`.

### msgType=2: `Fields` (init, then every 7 sim-days)

Wire version: **1 only**. Header = 16 bytes.

| Offset | Type | Field |
|---|---|---|
| 0 | `u8` | msgType = 2 |
| 1 | `u8` | version = 1 |
| 2 | `u16` | reserved |
| 4 | `u32` | fieldsVersion |
| 8 | `u32` | cellCount (`N`) |
| 12 | `u32` | reserved |

| Offset | Length | Field |
|---|---|---|
| 16 | `4*N` | `f32[N]` terrain |
| `16+4N` | `4*N` | `f32[N]` population |
| `16+8N` | `4*N` | `f32[N]` jobs |
| `16+12N` | `4*N` | `f32[N]` landValue |
| `16+16N` | `N` | `u8[N]` water |
| `16+17N` | `N` | `u8[N]` parks |

`N` = `fieldW * fieldH` from the latest `StaticCityJson`; this frame carries no
grid dimensions.

### msgType=3: `Traffic`

Wire version: **1 only**. Header = 32 bytes.

| Offset | Type | Field |
|---|---|---|
| 0 | `u8` | msgType = 3 |
| 1 | `u8` | version = 1 |
| 2 | `u16` | hotspotCount (`k`) |
| 4 | `u32` | w |
| 8 | `u32` | h |
| 12 | `f32` | cellSize |
| 16 | `f32` | originX |
| 20 | `f32` | originY |
| 24 | `u32` | valueCount |
| 28 | `u32` | reserved |

| Offset | Length | Field |
|---|---|---|
| 32 | `4*valueCount` | `f32[]` values |
| `32+4*valueCount` | `12*k` | `(f32 x, f32 y, f32 severity)[k]` |

Decoded and tested; `mf-state` does not mirror it into a resource
(`plugin.rs` leaves `Traffic` for direct consumers).

### msgType=4: `StaticMask` (0–3 frames after `ready`)

Wire version: **1 only**. Header = 12 bytes.

| Offset | Type | Field |
|---|---|---|
| 0 | `u8` | msgType = 4 |
| 1 | `u8` | version = 1 |
| 2 | `u8` | which (`0` water, `1` park, `2` building) |
| 3 | `u8` | reserved |
| 4 | `u32` | res |
| 8 | `u32` | reserved |

| Offset | Length | Field |
|---|---|---|
| 12 | `res*res` | `u8[res*res]` mask, row-major |

One frame per `has*Mask` flag that is `true` in `ready`. Procedural cities may
send zero. `mf-game`'s `Loading` gate waits for every flagged mask
(`CurrentCity::masks_complete`).

### msgType=5: `StaticBuildings` (sent once; additive)

Wire versions: **1 and 2** accepted on decode; encode always emits **version 2**.
Does **not** bump `PROTOCOL_VERSION` (`FromSimMsg::Buildings` / `binary.rs`
docs). Not a loading gate (`CurrentCity::masks_complete` ignores it).

Header = 12 bytes:

| Offset | Type | Field |
|---|---|---|
| 0 | `u8` | msgType = 5 |
| 1 | `u8` | version (`1` or `2`) |
| 2 | `u16` | reserved |
| 4 | `u32` | buildingCount |
| 8 | `u32` | vertexTotal (must equal sum of per-building `vertexCount`) |

Per building, fixed header then vertices:

**Version 1** header (4 bytes):

| Offset | Type | Field |
|---|---|---|
| 0 | `u8` | vertexCount (`3..=64` or decode error) |
| 1 | `u8` | flags (reserved; currently 0) |
| 2 | `u16` | heightDm |

**Version 2** header (6 bytes): version 1 header + trailing `u16 minHeightDm`.
On v1 decode, `min_height_dm` is filled as `0`.

Then `vertexCount` vertices × 4 bytes: `i16 xHalfM`, `i16 yHalfM` (LE).
Decode converts to meters: `x = xHalfM / 2.0`, `y = yHalfM / 2.0`.
`height_dm` / `min_height_dm` stay in decimeters (renderer converts).

`height_dm == 0` means "unknown; renderer may use density formula"
(`BuildingFootprint` docs).

### msgType=7: `StaticElevation` (sent once; additive)

Wire version: **1 only**. Optional real-DEM heightfield in **true meters**,
decoupled from the coarse `Fields.terrain` sim grid. Same delivery class as
msgType=5: does **not** bump `PROTOCOL_VERSION`; absence is valid; not a
loading gate (`CurrentCity::masks_complete` ignores it).

Header = 12 bytes:

| Offset | Type | Field |
|---|---|---|
| 0 | `u8` | msgType = 7 |
| 1 | `u8` | version = 1 |
| 2 | `u16` | reserved |
| 4 | `u32` | `res` (side length in cells) |
| 8 | `u32` | reserved |

| Offset | Length | Field |
|---|---|---|
| 12 | `2*res*res` | `i16[res*res]` heights in whole meters, row-major (row 0 = north/min-Z edge, matching masks) |

Sent once after `ready` when the city ships baked elevation (`simHost.ts` /
`encodeStaticElevation`). `mf-render/terrain.rs` prefers this channel over
normalized `Fields.terrain` when present.

---

## 3. Handshake, liveness, shutdown

```
client                                   sidecar
  |                                          |
  |  spawn (--port 0) + stdout handshake     |
  |  <--- {"mf":"sidecar","protocolVersion", |
  |        "port", "pid"}  (one stdout line) |
  |                                          |
  |----------------- WS connect -----------> |
  |  <---------------- hello ----------------|  HelloInfo
  |----------------- hello ----------------> |  {clientProtocolVersion}
  |     (abort if protocolVersion mismatch)  |
  |                                          |
  |----------------- init -----------------> |
  |  <---------------- ready ----------------|
  |  <------------ StaticMask x(0..3) -------|
  |  <---------- StaticBuildings? -----------|  optional, msgType=5
  |  <---------- StaticElevation? ----------|  optional, msgType=7
  |  <---------------- fields ---------------|
  |  <---------------- ui -------------------|  2 Hz
  |  <---------------- frame ----------------|  every 50 ms
  |                                          |
  |----------------- ping ------------------>|  every 5 s (mf-net)
  |  <---------------- pong -----------------|
  |                                          |
  |----------------- shutdown -------------->|
  |  <---------------- bye ------------------|
  X------------ socket closes --------------X
```

- The sidecar always sends its `hello` first, unprompted, immediately on connect.
- The client validates `protocolVersion === 1` and aborts the connection attempt on
  mismatch rather than trying to negotiate.
- **Liveness:** no inbound traffic (of any kind, including pongs) for **5 seconds**
  and the client declares the sim dead (`ws_transport.rs` `LIVENESS_WINDOW`).
  Process exit is detected immediately via `Child::try_wait` and distinguished
  from websocket silence in `SidecarDeathReason`. `mf-net`'s reconnect policy
  then respawns the sidecar and reconnects with backoff starting at 500 ms,
  doubling up to a 4 s cap, for up to **3 attempts** (`reconnect.rs`
  `MAX_ATTEMPTS`). Mid-game, recovery re-handshakes, restores from the latest
  autosave (or re-inits the current city), and resumes `InGame` under a
  "Reconnecting to simulation" overlay — it does not bounce to MainMenu. After 3
  failures the client shows a diagnostics screen (log tail + copy button).
- The client pings every **2.5 seconds** (`plugin.rs` `PING_INTERVAL`, half the
  silence window) so an idle menu screen does not spuriously look dead.
- **Clean shutdown:** the client sends `shutdown`; the sidecar stops its tick loop,
  replies `bye`, closes the socket, and exits with code 0. `SidecarProcess::drop` is
  the backstop: if the child doesn't exit within a reasonable window, it is killed
  directly.

Client state machine (`mf-game/src/state.rs`):

1. **Boot** — spawn sidecar, connect WS (`SimLink::spawn_and_connect`).
2. **ConnectingSim** — send client `hello`; on matching sidecar `hello` → MainMenu.
3. **Loading** — send `init` (or load-save path); gate on
   `masks_complete() && LatestFields && LatestUi` → InGame.
   Does **not** wait for `Frame`, `StaticBuildings`, `StaticElevation`, or `demand`.

Clean shutdown: client sends `shutdown`; sidecar replies `bye` and closes;
`SidecarProcess::drop` kills the child process group as backstop.

---

## 4. Sidecar process resolution (`mf-net/src/sidecar.rs`)

`SidecarProcess::spawn(headless_speed)` always appends `--port 0` (OS-assigned
port). Optional `--headless-speed <n>` if `headless_speed` is `Some`.

Stdout is piped; stderr inherited; stdin null. Handshake: one JSON line within
**15 s**:

| Field | JSON key | Type | Check |
|---|---|---|---|
| magic | `mf` | string | must equal `"sidecar"` |
| protocol | `protocolVersion` | `u32` | must equal `mf_protocol::PROTOCOL_VERSION` |
| listen port | `port` | `u16` | used for `ws://127.0.0.1:{port}` |
| process id | `pid` | `u32` | deserialized; unused |

### Binary / launch lookup order

1. **`$MF_SIDECAR_PATH`** — if set and the path `is_file()`, run that exact
   binary. If set but not a file, warn and fall through.
2. **Next to the running exe** — `{exe_dir}/metroforge-sidecar` (or
   `metroforge-sidecar.exe` on Windows).
3. **Dev fallback** — `bun run sidecar/index.ts` with
   `cwd = <repo>/sim` (resolved from the crate's `CARGO_MANIFEST_DIR`), only if
   `<repo>/sim/sidecar/index.ts` exists. Bun is resolved via `PATH`,
   else `$HOME/.bun/bin/bun`, else the string `"bun"`.

On Windows, spawn uses `CREATE_NO_WINDOW` so a second console does not appear.

---

## 5. Versioning and additive-fields policy

| Knob | Current value | Where |
|---|---|---|
| JSON handshake `protocolVersion` | `1` | `PROTOCOL_VERSION` in `lib.rs`; checked in `sidecar.rs` spawn handshake and `ConnectingSim` |
| Binary frame `version` byte | `1` for msgTypes 1–4 and 7; `1\|2` for msgType 5 | `binary.rs` |

**Bump `PROTOCOL_VERSION` when** a change would break an older peer talking to a
newer one: removing/reordering required JSON fields; changing binary layouts for
msgTypes 1–4; changing message semantics without a compatible shape.

**Do not bump for additive, optional data** (code-documented):

- JSON fields with `#[serde(default)]` / `Option` (e.g. `UiState` sim-depth
  fields, `UiRoute::{liveCrowding,operatingCost,farebox}`) — old sidecars stay
  parseable.
- **msgType=5 `StaticBuildings`** as a whole — optional; absence is valid;
  `FromSimMsg::Buildings` docs: does **not** bump `PROTOCOL_VERSION`.
- **msgType=5 wire version 2** — only adds trailing `minHeightDm` per building;
  v1 payloads still decode (`min_height_dm = 0`).
- **msgType=7 `StaticElevation`** — optional one-shot DEM heightfield; same
  additive policy as msgType=5.
- **v0.7 `UiState` weather fields** — `weatherState`, `weatherIntensity`,
  `weatherSeason`, `weatherEvent` (all `#[serde(default)]`).
- **v0.8 `trackCost.breakdown`** and the **`strataProbe`** request/response
  pair — optional; older peers omit them.

A client that receives a `hello` with an unrecognized `protocolVersion` aborts;
there is no negotiation in v1.
