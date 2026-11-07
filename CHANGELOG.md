# Changelog

**English** · [Русский](CHANGELOG.ru.md)

All notable changes to MMA are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow [Semantic Versioning](https://semver.org/); before `1.0.0` a breaking change raises the minor version.

Every entry states its impact on the three compatibility surfaces separately (see [README §9](README.md#9-compatibility-and-versioning)):

| Mark | Surface | Meaning |
|---|---|---|
| **Wire** | frame layout and `MMA_VERSION` | bytes on the connection |
| **Rust** | the `MMA` crate (`rlib`) | public items of `src/` |
| **ABI** | `libMMA` + `include/mma.h` | exported symbols, struct layouts, callback contracts |

## [Unreleased]

Changes since `v0.1.3` (commit `95a5abb`). Suggested version: **0.2.0** — the Rust push API is incompatible with 0.1.x, and `MMA_VERSION` stays `1` (frame layout and request semantics are unchanged by these edits).

### Changed

- **Channel delivery model — Rust, ABI (behaviour).** The shared `tokio::sync::broadcast` channel (1024 messages, filtered by `conn_id` in every connection) is replaced by `ChannelHub`: a table of one `mpsc` queue per channel connection, **256 messages** each.
  - A message for one `conn_id` goes only to that connection (hash lookup) instead of waking every channel connection.
  - Messages are no longer dropped with `Lagged`. When a queue is full the sender waits (`send_to_channel`) or gets an error (`try_send_to_channel`, C ABI).
  - An unknown `conn_id` is now an error (`"No such channel"`). A broadcast when no channel connection exists is now `Ok` (it was `Err`).
  - **Trade-off:** `send_to_channel(0, ..)` waits on each connection in turn. One client that does not read stalls the broadcast for all others. Measured on loopback with `examples/main_server`: one silent client, 64 KiB messages — a second client received 1322 of 3000 and then nothing, including a later small message.
- **Rust API (breaking).**
  - `ServerHandle::send_to_channel` is now `async fn(&self, id: u32, item: Response) -> Result<(), String>` (was a synchronous `Result<(), ()>`). Callers must `.await` it.
  - New `ServerHandle::try_send_to_channel(&self, id, item) -> Result<(), String>` that does not wait. With `id = 0` it tries every channel and returns `Partial broadcast: N/M channels accepted the message` on partial failure.
  - Removed `core::commands::ChannelSendedMessage`.
  - New public `core::server::ChannelHub` (`register`, `disconnect`, `send`, `try_send`).
- **C ABI (behaviour, signatures unchanged).** `mma_server_handle_send_to_channel` now uses the non-waiting path. It returns `MMA_SERVER_ERROR` for an unknown `conn_id`, a full queue (256) or a partly accepted broadcast. Callers that treated `MMA_OK` as "some subscriber exists" must review this.
- **Per-connection request limit removed.** The per-connection semaphore of 256 permits is now a one-permit semaphore that serialises connection-state decisions. The only in-flight cap is `max_in_flight` for the whole server, so one connection can use all of it.
- `ChannelConnect` with a wrong route: the `BadRequest` payload is now `"Route must match the opcode"` (was `"Route must be doesn't match by opcode"`). *Wire (text only)*
- `examples/main_server.rs` awaits `send_to_channel`.

### Fixed

- **`ChannelConnect` no longer allocates the channel before the route check** (known issue 1 of 0.1.3). A wrong route leaves nothing behind.
- **`CHANNEL_DISCONNECT` no longer gets a `NotFound` reply** (known issue 2 of 0.1.3). After the hook the channel is unregistered and the connection is closed without a response; the frame is not routed to channel handlers.
- **A closed connection unregisters its channel** on EOF, read error, shutdown and after `CHANNEL_DISCONNECT`. (The user-level disconnect hook still does not run on a dropped connection; see below.)
- **State race on a fresh connection.** The state decision for each frame is taken under a one-permit semaphore, so two frames can no longer both pass as `Fresh` (e.g. `Once` and `ChannelConnect`, or two `ChannelConnect`). Found by code reading; there is no automated race test.
- **Hook panics are contained.** A panic in the connect hook rolls back the channel registration and answers `InternalError`; a panic in the disconnect hook answers `InternalError`.
- A connect with an internal registration error answers `InternalError` instead of a half-open channel.
- Removed the debug `println!("connect, N")` on every channel connect.

### Known issues

1. A client that does not read stalls `send_to_channel(0, ..)` for everybody (see *Changed*). Use `try_send_to_channel`, or `tokio::time::timeout`, for broadcasts to untrusted clients.
2. `send_to_channel(0, ..)` stops at the first failing connection; the connections after it do not get the message.
3. The user's disconnect hook runs only for an explicit `CHANNEL_DISCONNECT`, not for a dropped TCP connection. A message that the hook sends to its own connection is lost.
4. A connect hook runs while the connection's state permit is held, so a slow hook delays the next frames of that connection.
5. One connection can occupy the whole `max_in_flight` budget (no per-connection cap).
6. No automated tests for the hub, the lifecycle hooks or the state race. The 14 existing tests pass. Verified manually: `cargo build --release`, `cargo test --locked --all-targets`, the C example compiles with `-std=c11 -Wall -Wextra -Werror`, and a live session against `examples/main_server` (wrong route → `BadRequest`; connect hook push; second connect → `Conflict`; `CHANNEL_DISCONNECT` → connection closed with no reply; `CHANNEL_DISCONNECT` on a fresh connection → `BadRequest`; the silent-client test above).
7. `Cargo.toml` has `version = "0.1.0"` while tags `v0.1.0`–`v0.1.3` exist. Tags `v0.1.0`–`v0.1.2` are on a separate line of history (Go bindings) and are not ancestors of `master`.
8. 17 source and workflow files use CRLF line endings. Add a `.gitattributes` (`* text=auto eol=lf`) so the diffs stay readable.

## [0.1.3] - 2026-11-02

Tag `v0.1.3` (commit `95a5abb`). Changes since commit `432905c` ("added README"). `Cargo.toml` still says `0.1.0` although the tags go up to `v0.1.3`, and `MMA_VERSION` is still `1`.

### Added

- **Connection lifecycle hooks** — user code that runs when a channel is opened and when a client closes it. *Rust, ABI*
  - Rust: `Router::register_on_channel_connect_handler(f)` and `Router::register_on_channel_disconnect_handler(f)`, with `f: Fn(Request, u32) -> impl Future<Output = ()>`. A repeated call replaces the hook. The dispatch methods `Router::on_channel_connect` / `on_channel_disconnect` are public.
  - C: `mma_server_register_on_channel_connect_handler` and `mma_server_register_on_channel_disconnect_handler` (same signature as `mma_server_register_channel_handler`, minus the route). Added to `mma.h` and exported by `libMMA`; the two symbols are the only ABI additions.
- **Reserved routes** `CHANNEL_CONNECT` and `CHANNEL_DISCONNECT`. `router::ReservedRoutes` is now `pub`, has a `CHANNEL_DISCONNECT` variant and implements `Display`. *Rust, Wire*
- `Request` implements `Clone`. *Rust*
- `examples/main_server.rs` shows both hooks (they push `Connect: …` / `Disconnect: …` messages to all channels).
- `mma_gui_client.py`: a Tkinter test client using only the Python standard library. It takes the full layout (`FramerConfig`), sends all three request opcodes, decodes responses and has a codec self-test.
- README (`README.md`, `README.ru.md`): §5.5 (reserved routes and hooks), hook examples for Rust and C, the new C functions, updated threading rules and repository layout. `CHANGELOG.md` and `CHANGELOG.ru.md`.
- GitHub Actions workflow `.github/workflows/native-release.yml`: on every push it runs `cargo test`, builds the release libraries, compiles the C example with `-Werror` and checks that `mma.h` compiles as C++. On `v*` tags it builds native packages for Windows x86-64, Linux x86-64, macOS x86-64 and macOS arm64 (library + `mma.h`, plus the MSVC import library on Windows, with SHA-256 files) and publishes a GitHub Release. The first version of the workflow compiled a `native-bridge/bridge.c` that is not on `master`; later commits removed that step.

### Changed

- **Wire — request semantics (not the frame layout):**
  - A `ChannelConnect` request must now carry the route `CHANNEL_CONNECT` (case-insensitive). Any other route gets `BadRequest` with the payload `"Route must be doesn't match by opcode"` and the connection stays `Fresh`. Clients that sent an arbitrary route must be updated.
  - A `ChannelMessage` with the route `CHANNEL_DISCONNECT` in `Channel` state runs the disconnect hook and then closes the connection (after the in-flight work drains). In `Fresh` state it gets `BadRequest` ("Channel not connected").
  - `MMA_VERSION` was **not** bumped. Decision for the maintainer: bump it to `2`, or accept the change within `1` and document it (done in README §5.5 and §9).
- **ABI — callback execution model (behaviour, not signatures):** C callbacks (routes, channel handlers, hooks) now run on Tokio's blocking thread pool instead of async worker threads. At most `min(max_in_flight, 128)` run at once; the rest wait for a slot, and the wait counts against `request_timeout`. A callback that outlives `request_timeout` still holds its slot, and `mma_server_stop` waits for running callbacks, so it can exceed `shutdown_grace`. Callbacks that assumed a particular thread, or that were sensitive to the previous parallelism, must be reviewed. *ABI*
- **ABI — callback argument views:** the `MMA_Request` view for channel callbacks and hooks is built from the request itself (its real route string); for ordinary routes the content is unchanged. A non-`MMA_OK` return from a channel callback or hook is logged to stderr; it is still not sent to the client. *ABI*
- **ABI — argument checks:** `valid_slice` now also rejects lengths above `isize::MAX`; the router initialisation error is printed to stderr before `mma_server_start` returns `MMA_SERVER_ERROR`. *ABI*
- Internal (private) changes in `src/network/network.rs`: the per-connection loop has a `disconnect` cancellation token, and `handle_cmd` takes an extra disconnect closure and returns the channel id from the connect closure. No public item changed. The route callback now moves the response into the blocking task, which also removes a response leak when the request future is dropped before it runs (from reading the code; not measured).
- Formatting and `// SAFETY:` comments in `src/c_api.rs`; the unused `Ipv4Addr` import is removed.

### Fixed

- A synchronous C callback no longer blocks a Tokio async worker (see *Changed → ABI*). This removes the main starvation risk named in the previous README ("callbacks run on worker threads").

### Known issues at this tag

Items 1 and 2 are fixed in *Unreleased* above; item 3 is still open.

1. `ChannelConnect` runs the connect closure (which allocates the channel subscription) **before** checking the route, so a wrong route can leave an unused subscription. Found by code reading, not measured.
2. After the disconnect hook the frame still goes to channel-handler routing. Without a handler named `CHANNEL_DISCONNECT` the client receives `NotFound` ("No such method") before the connection closes. Observed in a live test.
3. A dropped TCP connection does not run the disconnect hook; only the explicit `CHANNEL_DISCONNECT` frame does.
4. Reserved routes are compared with `to_uppercase()` on each request (case-insensitive, allocating). The empty `if` in the `Fresh`/`ChannelMessage` branch is dead code.
5. Hooks and the reserved-route checks have no automated tests. Verified manually: `cargo build --release`, `cargo test` (14 tests pass, unchanged), the C example compiles against the new header, all 20 `mma_*` functions of the header are exported, and a live session against `examples/main_server` (wrong route → `BadRequest`; connect hook → push `Connect: …`; second connect → `Conflict`; disconnect → hook push, `NotFound`, close after ≈ 4 s with `drain_timeout` 2 s; `CHANNEL_DISCONNECT` on a fresh connection → `BadRequest`).
6. Line endings: 18 files in the working tree use CRLF (a content-free change against the commit). Add a `.gitattributes` (`* text=auto eol=lf`) before committing so the diff stays readable.

[Unreleased]: https://github.com/Edestuss0/MMA-Network-protocol/compare/v0.1.3...HEAD
[0.1.3]: https://github.com/Edestuss0/MMA-Network-protocol/compare/432905c...v0.1.3
