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

Changes since `v0.1.4` (commit `b6b8579`). Suggested version: **0.2.0** — module paths and `Config` changed (Rust), and `MMA_ServerConfig` changed its size (C ABI). `MMA_VERSION` stays `1`: the frame layout and request semantics are unchanged; TLS wraps the unchanged byte stream.

### Added

- **TLS (server side)** on top of `tokio-rustls` 0.26 / rustls, without client certificates. *Rust, ABI*
  - `Config` adds one field: `tls_config: Option<TlsConfig>`. `None` disables TLS; `Some(TlsConfig { tls_certificate_path, tls_key_path, tls_handshake_timeout })` enables it.
  - The C ABI adds `MMA_TLSConfig` (certificate path, private-key path and handshake timeout in milliseconds) and one `const MMA_TLSConfig *tls_config` field at the end of `MMA_ServerConfig`. The Rust `repr(C)` size of `MMA_ServerConfig` is 72 bytes on x86-64 (up from 64 bytes); `MMA_TLSConfig` is 24 bytes.
  - A connection that fails the handshake, or does not finish it within `tls_handshake_timeout`, is dropped and logged.
  - `ServerHandle::change_config` (and `mma_server_handle_change_config`) refuses a change of any TLS field: `InvalidInput` in Rust, `MMA_SERVER_ERROR` in C.
- Four runnable server examples: `examples/example_no_tls.rs` and `examples/example_with_tls.rs` for Rust, plus `examples/example_no_tls.c` and `examples/example_with_tls.c` for the C ABI. The TLS examples accept certificate and private-key paths as command-line arguments. No certificates or private keys are bundled.
- `mma_gui_client.py` has a TLS section (transport security, certificate check, server name).

### Changed

- **Rust API (breaking).**
  - `Config`, `FramerConfig`, `BigBytesConfig`, `ConfigError` and `MMA_VERSION` moved from `MMA::core::config` to `MMA::config::config`.
  - `ServerHandle` moved from `core::server` to `core::handle`. `ChannelHub` is now private (`pub(crate)` module); it was reachable through `core::server` in 0.1.4.
  - `SocketStatus` moved from the private network module to `core::commands` and is public.
  - `Config` has four new fields, so struct literals without `..Config::default()` no longer compile.
  - `examples/main_server.rs` is replaced by the Cargo examples `no_tls` and `with_tls`; `cargo run --example main_server` no longer works. The C examples are `examples/example_no_tls.c` and `examples/example_with_tls.c` (the old `examples/main_server.c` reference is obsolete).
- `serve_connection` is generic over the stream (`AsyncConnection`: `AsyncRead + AsyncWrite + Unpin + Send`), so TLS and plain TCP share one code path. Internal.
- README: TLS, current Rust/C example names and build commands, all four native release targets, new module paths, the 72-byte `MMA_ServerConfig`, SIGPIPE rule, and duplicate-route behaviour of the C API. The earlier statement that `FramerConfig::new()` exists was wrong; the constructor is `FramerConfig::default()` (`BigBytesConfig::new()` exists).
- **CI and native release pipeline.** On pushes, pull requests and manual runs, CI runs `cargo test --locked --all-targets`, builds the release library and both Rust examples, compiles both C examples with C11 plus `-Wall -Wextra -Werror`, and checks that `mma.h` is valid C++17. On `v*` tags, the release matrix builds Windows x86-64, Linux x86-64, macOS Intel x86-64 and macOS Apple Silicon arm64 packages. Each package contains the native library and `mma.h`; the Windows package also contains the MSVC import library. SHA-256 checksum files are generated and verified before publishing the GitHub Release. *CI/release only; no Wire, Rust API or ABI change.*

### Known issues

Verified by running the code (`cargo build --release`, `cargo test --locked --all-targets`: 14 tests pass; live sessions against `examples/with_tls`, `examples/no_tls` and a C program covering all 20 exported functions).

1. **TLS initialization now fails explicitly.** `Server::from_listener` returns an `io::Error` if the certificate chain or private key cannot be loaded; the C API reports `MMA_SERVER_ERROR` from `mma_server_start`. It no longer falls back to plain TCP after TLS initialization fails. *Fixed in the current source tree.*
2. **`include/mma.h` uses `MMA_TLSConfig` before declaring the typedef.** `MMA_ServerConfig` contains `const MMA_TLSConfig *tls_config`, but the typedef appears below the structure and there is no forward declaration. As written, including the header fails in both C and C++; move the `MMA_TLSConfig` definition above `MMA_ServerConfig` or add a valid named forward declaration. The comment above the field also still mentions the removed `tls_enable` flag; TLS is now controlled by whether `tls_config` is `NULL`. The C-example compile and C++ header-smoke steps in CI will remain red until the header is fixed.
3. **C hosts are killed by `SIGPIPE`.** Writing to a socket whose peer has closed raises `SIGPIPE`; a C host does not ignore it (a Rust binary does). Observed: the host process exited with status 13 when a client with a full push queue disconnected. Workaround: `signal(SIGPIPE, SIG_IGN)` before `mma_server_start`. Suggested fix: ignore the signal inside the library or send with `MSG_NOSIGNAL`.
4. In the C API a duplicate route passes `mma_server_register_route` (returns `MMA_OK`) and fails only at `mma_server_start` with `MMA_SERVER_ERROR`; the reason goes to stderr.
5. `Config::validate` ignores the TLS fields.
6. The header comment said callbacks run "on runtime worker threads"; they run on Tokio's blocking pool (corrected in the provided header).
7. No automated tests cover TLS, `ChannelHub`, the lifecycle hooks or the C ABI.
8. TLS credentials are not bundled. Supply a certificate and matching private key when running either TLS example; keep private keys outside the repository and exclude them from version control.
9. The earlier 0.1.3 and 0.1.4 known issues that remain: the user's disconnect hook does not run when a TCP connection is dropped; `send_to_channel(0, ..)` is blocked by one non-reading client; no per-connection in-flight cap.
10. Compiler warnings: crate name `MMA` is not snake case, `ReservedRoutes` variants are upper case, two unused imports.
11. Source files use CRLF line endings; `.gitattributes` with `* text=auto eol=lf` is provided.

## [0.1.4] - 2026-11-07

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


[Unreleased]: https://github.com/Edestuss0/MMA-Network-protocol/compare/v0.1.4...HEAD
[0.1.4]: https://github.com/Edestuss0/MMA-Network-protocol/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/Edestuss0/MMA-Network-protocol/compare/432905c...v0.1.3