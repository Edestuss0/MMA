# Changelog

**English** · [Русский](CHANGELOG.ru.md)

All notable changes to MMA are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow [Semantic Versioning](https://semver.org/); before `1.0.0` a breaking change raises the minor version.

Every entry states its impact on the three compatibility surfaces separately (see [README §9](README.md#9-compatibility-and-versioning)):

| Mark | Surface | Meaning |
|---|---|---|
| **Wire** | frame layout and `MMA_VERSION` | bytes on the connection |
| **Rust** | the `MMA` crate (`rlib`) | public items of `src/` |
| **ABI** | `libMMA` + `include/mma.h` | exported symbols, struct layouts, callback contracts |

### Added

- **Opaque request handles and explicit request lifetime.** Added `MMA_Request` as an opaque handle and `mma_request_retain` / `mma_request_release` to keep a request alive after its callback returns. Added `mma_request_get_info`, indexed option access (`mma_request_option_count`, `mma_request_option_at`) and `mma_request_is_cancelled`. *ABI*
- **Explicit asynchronous route completion.** Route callbacks now receive `MMA_Request *` and return `void`; C constructs a response using `mma_response_new`, setters and `mma_request_complete`. Completion can occur after the callback returns, provided the request was retained. Added `MMA_REQUEST_ALREADY_COMPLETED` and `MMA_REQUEST_CANCELLED`. *ABI*
- **Opaque response objects.** `MMA_Response` is now library-owned and opaque; payload and option setters copy the supplied buffers. `mma_request_complete` documents response ownership on success, duplicate completion and cancellation. *ABI*
- **Current C examples.** Both C examples demonstrate the opaque-handle API, route completion, channel/lifecycle notifications, `MMA_ServerHandle` and shutdown. The TLS example uses milliseconds for `tls_handshake_timeout`. *ABI / examples*
- Added `.gitattributes` with LF normalization for text files to keep cross-platform diffs stable. *Repository / tooling*

### Changed

- **C ABI — breaking redesign.** `MMA_Request` and `MMA_Response` are no longer public by-value structs. The old `MMA_Option`, `MMA_ChannelHandlerCallback`, callback-returned `MMA_Response`, `mma_server_route` alias and `MMA_RESPONSE_MESSAGE` name are removed/replaced. New callbacks return `void`; channel/lifecycle callbacks receive `MMA_Request *`, `connection_id` and mutable `user_data`. Recompile all C and C++ consumers with the current `include/mma.h`. *ABI*
- **C callback execution model.** Callbacks are called synchronously from the server's Tokio runtime tasks; they are not dispatched to `spawn_blocking`. A blocking C callback occupies a Tokio worker and cannot be forcibly interrupted by `request_timeout`. Applications that need blocking I/O or CPU work must retain the request and dispatch work to their own bounded executor. *ABI / behaviour*
- **Channel callbacks are notifications.** They do not produce a route response. `mma_request_complete` on a channel/lifecycle request returns `MMA_INVALID_ARGUMENT`, and the caller retains ownership of the response. *ABI*
- **Request/response ownership.** Each successful retain must be balanced by one release. Request-info and option pointers are borrowed and remain valid only while a request reference is held. Completion consumes the response for valid route requests on `MMA_OK`, `MMA_REQUEST_ALREADY_COMPLETED` and `MMA_REQUEST_CANCELLED`; on `MMA_INVALID_ARGUMENT`, the caller retains it. *ABI*
- **Options.** Duplicate option keys are now accepted by the encoder and decoder. Empty keys/values, truncated entries, wrong counts and trailing option bytes remain invalid. Applications that depended on uniqueness must enforce it themselves. *Wire (decoder validation)*
- **Framer encoder.** It now reserves the estimated frame capacity and explicitly extends the destination buffer to the header length before writing header fields. The Framer also starts with a cached copy of the current configuration. *Internal / performance and correctness*
- **Workflow.** CI now compiles the public C examples on Linux and Windows/MSVC and checks the header as C++ using opaque pointers rather than attempting to instantiate opaque request/response types. Native release packages remain available for Windows x86-64, Linux x86-64, macOS x86-64 and macOS arm64. *CI/release only*

### Fixed


- Corrected the TLS example's `tls_handshake_timeout` value to milliseconds (`10000` = 10 seconds).
- C examples ignore `SIGPIPE` on POSIX systems before serving connections, so a peer closing during a write does not terminate the example host process.
- Removed the unused `AHashSet` import from the Framer.
- Corrected the C API implementation comment that previously described callbacks as running away from the runtime thread; callback invocation is synchronous on Tokio workers.

### Migration notes

- Rebuild every C/C++ consumer with the new `include/mma.h` and link it to the matching `libMMA` binary. Do not mix a header from the previous snapshot with the current shared library.
- Replace by-value request access with `mma_request_get_info` and `mma_request_option_count` / `mma_request_option_at`.
- Replace route callbacks returning `MMA_Response` with `void` callbacks that call `mma_request_complete`.
- Use `mma_request_retain` before handing a request to another thread or executor; pair every successful retain with exactly one release.
- `mma_server_get_handle` is valid only after `mma_server_start` succeeds. Destroy the returned handle before destroying the server.

### Remaining limitations

- A synchronous C callback cannot be interrupted. If it blocks, it delays the Tokio worker currently executing it and may delay server shutdown.
- The C ABI has no runtime ABI-version query or structured last-error string; check return codes and stderr diagnostics.
- The user disconnect hook is not invoked for a peer that disappears without sending the explicit `CHANNEL_DISCONNECT` frame.
- `send_to_channel(0, ..)` uses a non-waiting per-channel queue path from the C handle API but reports an error if a broadcast is only partially accepted; it does not provide durable delivery or retry semantics.
- The `Cargo.toml` package version remains `0.1.0` in this source snapshot, while the Git tags reach `v0.1.4`. Update the crate version deliberately when preparing the next release.

## [0.1.4] - 0.1.5

Tag `v0.1.4` (commit `b6b8579`). Changes since `v0.1.3` (commit `95a5abb`). `MMA_VERSION` stays `1`; `Cargo.toml` still says `0.1.0`.

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


[Unreleased]: https://github.com/Edestuss0/MMA-Network-protocol/compare/v0.1.4...HEAD
[0.1.4]: https://github.com/Edestuss0/MMA-Network-protocol/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/Edestuss0/MMA-Network-protocol/compare/432905c...v0.1.3