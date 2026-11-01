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

Baseline: commit `432905c` ("added README"). Candidate version: **0.2.0**, because request semantics changed (see *Changed → Wire*). `Cargo.toml` still says `0.1.0`, and `MMA_VERSION` is still `1`.

### Added

- **Connection lifecycle hooks** — user code that runs when a channel is opened and when a client closes it. *Rust, ABI*
  - Rust: `Router::register_on_channel_connect_handler(f)` and `Router::register_on_channel_disconnect_handler(f)`, with `f: Fn(Request, u32) -> impl Future<Output = ()>`. A repeated call replaces the hook. The dispatch methods `Router::on_channel_connect` / `on_channel_disconnect` are public.
  - C: `mma_server_register_on_channel_connect_handler` and `mma_server_register_on_channel_disconnect_handler` (same signature as `mma_server_register_channel_handler`, minus the route). Added to `mma.h` and exported by `libMMA`; the two symbols are the only ABI additions.
- **Reserved routes** `CHANNEL_CONNECT` and `CHANNEL_DISCONNECT`. `router::ReservedRoutes` is now `pub`, has a `CHANNEL_DISCONNECT` variant and implements `Display`. *Rust, Wire*
- `Request` implements `Clone`. *Rust*
- `examples/main_server.rs` shows both hooks (they push `Connect: …` / `Disconnect: …` messages to all channels).
- `mma_gui_client.py`: a Tkinter test client using only the Python standard library. It takes the full layout (`FramerConfig`), sends all three request opcodes, decodes responses and has a codec self-test.
- README (`README.md`, `README.ru.md`): §5.5 (reserved routes and hooks), hook examples for Rust and C, the new C functions, updated threading rules, limitations and repository layout. `CHANGELOG.md` and `CHANGELOG.ru.md`.

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

### Known issues in this change

These exist in the current working tree and are documented in README §11. They are not fixed here.

1. `ChannelConnect` runs the connect closure (which allocates the channel subscription) **before** checking the route, so a wrong route can leave an unused subscription. Found by code reading, not measured.
2. After the disconnect hook the frame still goes to channel-handler routing. Without a handler named `CHANNEL_DISCONNECT` the client receives `NotFound` ("No such method") before the connection closes. Observed in a live test.
3. A dropped TCP connection does not run the disconnect hook; only the explicit `CHANNEL_DISCONNECT` frame does.
4. Reserved routes are compared with `to_uppercase()` on each request (case-insensitive, allocating). The empty `if` in the `Fresh`/`ChannelMessage` branch is dead code.
5. Hooks and the reserved-route checks have no automated tests. Verified manually: `cargo build --release`, `cargo test` (14 tests pass, unchanged), the C example compiles against the new header, all 20 `mma_*` functions of the header are exported, and a live session against `examples/main_server` (wrong route → `BadRequest`; connect hook → push `Connect: …`; second connect → `Conflict`; disconnect → hook push, `NotFound`, close after ≈ 4 s with `drain_timeout` 2 s; `CHANNEL_DISCONNECT` on a fresh connection → `BadRequest`).
6. Line endings: 18 files in the working tree use CRLF (a content-free change against the commit). Add a `.gitattributes` (`* text=auto eol=lf`) before committing so the diff stays readable.

[Unreleased]: https://github.com/Edestuss0/MMA-Network-protocol/compare/432905c...HEAD
