<div align="center">

# MMA Network Protocol

**A binary TCP protocol with a configurable wire format — Rust core, C ABI.**

**English** · [Русский](README.ru.md)

</div>

MMA is a private, binary, request/response and channel protocol over TCP. Its defining feature: **there is no single fixed wire format.** The byte layout of a frame is defined by a configuration (`FramerConfig`) that both peers share, so two deployments with different configurations look like two different, incompatible protocols on the wire.

This document specifies the frame format, the connection model, the Rust API and the C ABI.

## Contents

- [1. Overview](#1-overview)
- [2. Quick start](#2-quick-start)
- [3. Frame format](#3-frame-format)
- [4. Options](#4-options)
- [5. Routes and connection model](#5-routes-and-connection-model)
- [6. Errors](#6-errors)
- [7. Rust API](#7-rust-api)
- [8. C ABI](#8-c-abi)
- [9. Compatibility and versioning](#9-compatibility-and-versioning)
- [10. Security](#10-security)
- [11. Repository layout](#11-repository-layout)

---

## 1. Overview

### 1.1. The idea: no standard wire format

Most protocols (HTTP, RESP, MQTT, gRPC) fix their byte layout in a specification, so any sniffer can recognise them by signature. MMA specifies the **rules**, not the layout. The layout is chosen per deployment:

| `FramerConfig` field | What it defines |
|---|---|
| `header_length` | header size, 17–255 bytes |
| `opcode_pos`, `version_pos`, `route_len_pos`, `options_count_pos` | which header byte holds each one-byte field |
| `big_bytes_config` | which of the three 4-byte slots at the end of the header holds `options_len`, `payload_len`, `req_id` |
| `route_order`, `options_order`, `payload_order` | order of the body sections (a permutation of 1..3) |
| `options_key_first` | key/value order inside an option |
| spare header bytes | zero on encode, ignored on decode |

Consequences:

- A frame has no constant signature (no magic number); field positions and section order differ between deployments. This makes naive pattern matching harder, but it is **obfuscation, not a security mechanism**.
- The configuration is a **shared wire profile** that client and server must agree on out of band. Both sides must use identical values.
- The **logical model** is constant: `opcode`, `version`, `req_id`, `route`, `options`, `payload`.
- MMA provides **no encryption and no authentication**. Run it over TLS or inside a trusted network (see [Security](#10-security)).
- There is no handshake yet: if the configurations differ, the server misparses frames — typically answering `BadRequest` and closing the connection.

### 1.2. Purpose

MMA is meant for connecting your own clients and servers when you want:

- compact framing with explicit lengths;
- a wire format you can tune per deployment;
- two interaction modes — **Once** (request → one response) and **Channel** (persistent connection with server push);
- one server engine usable from Rust and C.

Out of scope: encryption, authentication, service discovery, compatibility with third-party clients, delivery guarantees across restarts. The crate does **not** ship a client; build one from this specification.

### 1.3. Properties

| Property | Value |
|---|---|
| Transport | TCP |
| Interaction | Request/Response (`Once`) and `Channel` |
| Encoding | Binary, layout set by `FramerConfig` |
| Wire version | `MMA_VERSION = 1` |
| Header size | min 17 bytes, default 29 |
| Max frame size | `header_length + max_message_length` (default `29 + 10 000 000`) |
| Integer byte order | big-endian |
| Interfaces | Rust (server API), C (`include/mma.h`) |

---

## 2. Quick start

### Rust server

```toml
[dependencies]
MMA = { git = "https://github.com/Edestuss0/MMA-Network-protocol" }
tokio = { version = "1", features = ["full"] }
bytes = "1"
```

```rust
use MMA::config::config::Config;
use MMA::core::{commands::{Request, Response}, router::Router, server::Server};
use bytes::Bytes;

async fn upper(req: Request, mut res: Response) -> Response {
    res.payload = Bytes::from(req.payload.to_ascii_uppercase());
    res
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut server = Server::new(Config::default()).await?;
    let mut router = Router::new();
    router.register_route("UP".to_string(), upper).expect("route");
    server.run(router).await
}
```

The repository includes two Rust examples. Run the plain TCP server with `cargo run --locked --example no_tls`. The TLS server accepts a PEM certificate and private-key path: `cargo run --locked --example with_tls -- path/to/server.crt path/to/server.key`. Both examples listen on `0.0.0.0:8080`. The repository does not include a production certificate or private key; provide your own test or deployment credentials.

### Minimal client (Python, default layout)

```python
import socket, struct

HEADER_LEN = 29

def recv_exact(sock, size):
    buf = bytearray()
    while len(buf) < size:
        part = sock.recv(size - len(buf))
        if not part:
            raise EOFError("connection closed")
        buf.extend(part)
    return bytes(buf)

def make_frame(opcode, route, payload=b"", req_id=1):
    h = bytearray(HEADER_LEN)
    h[0], h[1], h[2], h[3] = opcode, 1, len(route), 0   # opcode, version, route_len, options_count
    h[17:29] = struct.pack(">III", 0, len(payload), req_id)  # options_len, payload_len, req_id
    return bytes(h) + route + payload

with socket.create_connection(("127.0.0.1", 8080)) as s:
    s.sendall(make_frame(2, b"UP", b"hello", req_id=1))
    h = recv_exact(s, HEADER_LEN)
    payload_len = struct.unpack(">I", h[21:25])[0]
    print(h[0], recv_exact(s, payload_len))   # 1 b'HELLO'
```

### C examples

The repository contains two C implementations of the same server: `example_no_tls.c` (plain TCP) and `example_with_tls.c` (TLS). Build the shared library and both examples on Linux with a C11 compiler:

```bash
cargo build --locked --release --lib

for source in examples/example_no_tls.c examples/example_with_tls.c; do
    name="$(basename "${source%.c}")"
    cc \
        -std=c11 \
        -D_POSIX_C_SOURCE=200809L \
        -Wall -Wextra -Wpedantic -Werror \
        -I include \
        "$source" \
        -L target/release \
        -Wl,-rpath,"$PWD/target/release" \
        -lMMA -pthread \
        -o "target/$name"
done
```

Run the plain TCP example with `./target/example_no_tls`. The TLS example requires paths to a certificate and private key: `./target/example_with_tls path/to/server.crt path/to/server.key`. The commands above use the Linux/GNU linker; use the corresponding compiler and dynamic-library search-path options on Windows or macOS.

Both C programs use the public header [`include/mma.h`](include/mma.h). See [8. C ABI](#8-c-abi).

---

## 3. Frame format

### 3.1. Layout

```text
┌──────────────────────────────────────────────────────────────┐
│ Header — header_length bytes (≥ 17, default 29)              │
│   opcode / version / route_len / options_count               │
│       (one byte each, at configured positions)               │
│   spare bytes (zero)                                         │
│   last 12 bytes: three 4-byte big-endian slots holding       │
│       options_len, payload_len, req_id                       │
│       (slot order set by big_bytes_config)                   │
├──────────────────────────────────────────────────────────────┤
│ Body — three sections in the order set by                    │
│        route_order / options_order / payload_order           │
│   Route    route_len bytes (UTF-8)                           │
│   Options  options_len bytes                                 │
│   Payload  payload_len bytes                                 │
└──────────────────────────────────────────────────────────────┘
```

Responses carry no route section (`route_len = 0`).

### 3.2. Header fields

Offsets are 0-based from the start of the frame; `H = header_length`. Configured positions (`*_pos`) are 1-based, so offset = `pos − 1`. The offsets of the three 4-byte fields below are for the default slot order.

| Field | Offset | Size | Type | Meaning | Validation |
|---|---:|---:|---|---|---|
| `opcode` | `opcode_pos − 1` (def. 0) | 1 | `u8` | Request or response type ([3.4](#34-opcodes)) | request: 1, 2 or 3 |
| `version` | `version_pos − 1` (def. 1) | 1 | `u8` | Protocol version, currently `1` | checked by the router |
| `route_len` | `route_len_pos − 1` (def. 2) | 1 | `u8` | Route length; `0` in responses | 0–255, route must be UTF-8 |
| `options_count` | `options_count_pos − 1` (def. 3) | 1 | `u8` | Number of options | 0–255, must match parsed count |
| spare | rest of `[0, H−12)` | `H − 16` | — | Reserved for layout variation | zero on encode, ignored on decode |
| `options_len` | `H − 12` (def.) | 4 | `u32` BE | Size of the options section | counts toward `max_message_length` |
| `payload_len` | `H − 8` (def.) | 4 | `u32` BE | Size of the payload | counts toward `max_message_length` |
| `req_id` | `H − 4` (def.) | 4 | `u32` BE | Request identifier, echoed in the response | any `u32` |

What each field is for:

- **`opcode`** — what the frame is and how it is handled.
- **`version`** — protocol version; lets semantics evolve without changing the layout.
- **`route_len`** — boundary of the route section; the route selects the server handler.
- **`options_count`** — number of key/value pairs; together with `options_len` it validates the section.
- **`options_len`, `payload_len`** — section sizes; they determine the frame length.
- **`req_id`** — correlates responses with requests. Responses may arrive out of order, so use unique `req_id`s among in-flight requests.
- **spare bytes** — free header space, used to vary the layout; carry no meaning.

### 3.3. Frame length

```text
body_len  = route_len + options_len + payload_len    (checked addition)
frame_len = header_length + body_len                 (checked addition)
```

`body_len` is checked against `max_message_length` before any allocation. A frame can arrive in several TCP reads, and several frames can arrive in one; boundaries come only from the length fields.

### 3.4. Opcodes

**Request opcodes (client → server)**

| Name | Code | Valid in state | Purpose |
|---|---:|---|---|
| `ChannelConnect` | 1 | `Fresh` | Switch the connection to Channel mode; the route **must** be `CHANNEL_CONNECT` ([§5.5](#55-reserved-routes-and-connection-lifecycle-hooks)) |
| `Once` | 2 | `Fresh`, `Once` | Request handled by a route handler; one response |
| `ChannelMessage` | 3 | `Channel` | Message for a channel handler; no response on success. The route `CHANNEL_DISCONNECT` is reserved ([§5.5](#55-reserved-routes-and-connection-lifecycle-hooks)) |

**Response opcodes (server → client)**

| Name | Code | Meaning | Sent when |
|---|---:|---|---|
| `Ok` | 1 | Success | Default status of a response |
| `BadRequest` | 2 | Invalid request | Decode error; wrong version; channel message without a channel; `ChannelConnect` with a route other than `CHANNEL_CONNECT` |
| `Unauthorized` | 3 | Not authorised | Set by your handler |
| `Forbidden` | 4 | Forbidden | Set by your handler |
| `NotFound` | 5 | Unknown route | Payload `"No such method"` |
| `Conflict` | 6 | State conflict | Second `ChannelConnect` |
| `InternalError` | 7 | Server error | Handler panic |
| `ChannelMessage` | 8 | Server push | Delivered via `send_to_channel` / `try_send_to_channel` |
| `Shutdown` | 9 | Server stopping | Reserved; not currently sent |
| `ChannelConnected` | 10 | Channel established | Reply to `ChannelConnect` |
| `Timeout` | 11 | Handler timed out | `request_timeout` expired |

Codes `0` and `≥ 12` are undefined. An unknown request opcode is a decode error.

### 3.5. Example frames

Default layout (`header_length = 29`, positions 1/2/3/4, order route → options → payload).

Request: `Once`, route `GET`, payload `hello`, `req_id = 1` — 37 bytes:

```text
02 01 03 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 05 00 00 00 01 47 45 54 68 65 6c 6c 6f
```

| Offset | Hex | Field | Value |
|---:|---|---|---|
| 0 | `02` | `opcode` | `Once` |
| 1 | `01` | `version` | 1 |
| 2 | `03` | `route_len` | 3 |
| 3 | `00` | `options_count` | 0 |
| 4–16 | `00 …` | spare | 13 zero bytes |
| 17–20 | `00 00 00 00` | `options_len` | 0 |
| 21–24 | `00 00 00 05` | `payload_len` | 5 |
| 25–28 | `00 00 00 01` | `req_id` | 1 |
| 29–31 | `47 45 54` | route | `"GET"` |
| 32–36 | `68 65 6c 6c 6f` | payload | `"hello"` |

Response `Ok` — 34 bytes:

```text
01 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 05 00 00 00 01 68 65 6c 6c 6f
```

The same logical request under another configuration (for example `version_pos=4`, `route_len_pos=6`, `options_count_pos=7`, order payload → options → route, as in `examples/example_no_tls.rs`) produces entirely different bytes. That is the point of the design.

### 3.6. Layout configuration

| Field | Allowed values | Default | Effect on the wire |
|---|---|---|---|
| `header_length` | 17–255 | 29 | header size, position of the 12-byte tail |
| `opcode_pos`, `version_pos`, `route_len_pos`, `options_count_pos` | 1…`header_length − 12`, pairwise distinct | 1, 2, 3, 4 | offsets of the one-byte fields |
| `route_order`, `options_order`, `payload_order` | permutation of 1..3 | 1, 2, 3 | order of body sections |
| `big_bytes_config.options_len_order` / `payload_len_order` / `req_id_order` | permutation of 1..3 | 1, 2, 3 | slot of each 4-byte field in the tail |
| `options_key_first` | `true` / `false` | `true` | `key,value` or `value,key` inside an option |
| `max_message_length` | `u32` | 10 000 000 | not a layout parameter; a server-side size limit |

All fields except `max_message_length` are wire parameters and **must match on both sides**.

### 3.7. Malformed and incomplete frames

| Situation | Server reaction | Connection |
|---|---|---|
| Incomplete header or body | waits for more data | stays open |
| EOF in the middle of a frame | no response | closed |
| Length above `max_message_length` | `BadRequest` | closed |
| Length overflow | `BadRequest` | closed |
| Unknown request opcode | `BadRequest` | closed |
| Route is not valid UTF-8 | `BadRequest` | closed |
| Malformed options (empty key/value, truncated data, trailing bytes, wrong count) | `BadRequest` | closed |
| Wrong `version` | `BadRequest` (`"Incorrect protocol version"`) | stays open |

After a decode error the stream cannot be resynchronised, so the connection is closed. `BadRequest` carries the `req_id` from the header when it could be read, otherwise `0`.

---

## 4. Options

Options are named parameters attached to a frame (metadata, flags, trace IDs). The protocol reserves no keys; their meaning belongs to your application.

| Property | Behaviour |
|---|---|
| Key | raw bytes, case-sensitive, 1–65535 bytes |
| Value | raw bytes, 1–65535 bytes |
| Empty key or value | rejected |
| Duplicate keys | allowed; preserve insertion order |
| Count | 0–255 per frame |
| Unknown keys | ignored by the library; available in `Request.options` |

Wire format of one option (`options_count` of them are concatenated into the options section):

```text
options_key_first = true :  [key_len u16 BE][value_len u16 BE][key][value]
options_key_first = false:  [key_len u16 BE][value_len u16 BE][value][key]
```

Example — `POW`, payload `7`, option `pow = 3`, `req_id = 2`, default layout (41 bytes):

```text
02 01 03 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 08 00 00 00 01 00 00 00 02 50 4f 57 00 03 00 01 70 6f 77 33 37
```

| Section | Bytes | Meaning |
|---|---|---|
| route (3) | `50 4f 57` | `"POW"` |
| options (8) | `00 03` `00 01` `70 6f 77` `33` | `key_len=3`, `value_len=1`, key `pow`, value `3` |
| payload (1) | `37` | `"7"` |

Header: `options_count = 1`, `options_len = 8`, `payload_len = 1`, `req_id = 2`.

Rust access: `Request::get_option(&self, key: &[u8]) -> Option<Bytes>` (no data copy). Values are raw bytes; parse them yourself and **never `unwrap` client-supplied values**. Typed accessors are not implemented.

---

## 5. Routes and connection model

### 5.1. Routes

- A route is a UTF-8 string of at most 255 bytes, case-sensitive.
- Unknown route → `NotFound` (`"No such method"`); the connection stays open.
- Registering the same route twice (also across request and channel handlers) is an error.
- Two route names are **reserved** for the channel lifecycle: `CHANNEL_CONNECT` and `CHANNEL_DISCONNECT` ([§5.5](#55-reserved-routes-and-connection-lifecycle-hooks)). They are matched case-insensitively.

### 5.2. Connection states

```text
            ChannelConnect                     ChannelConnect
 Fresh ──────────────────────► Channel ◄──────────────────────┐ (Conflict, stays)
   │                             │  ▲                          │
   │ Once                        │  └── ChannelMessage ───────┘ (no response)
   ▼                             │
 Once ◄── Once (loop)            └── Once → mismatch (stays)
```

| State | Request | Result | New state |
|---|---|---|---|
| `Fresh` | `Once` | route handler | `Once` |
| `Fresh` | `ChannelConnect`, route `CHANNEL_CONNECT` | `ChannelConnected`; connect hook runs | `Channel` |
| `Fresh` | `ChannelConnect`, any other route | `BadRequest` ("Route must match the opcode") | `Fresh` |
| `Fresh` | `ChannelMessage` | `BadRequest` ("Channel not connected") | `Fresh` |
| `Once` | `Once` | route handler | `Once` |
| `Once` | `ChannelConnect` / `ChannelMessage` | `BadRequest` (type mismatch) | `Once` |
| `Channel` | `ChannelConnect` | `Conflict` | `Channel` |
| `Channel` | `ChannelMessage` | channel handler | `Channel` |
| `Channel` | `ChannelMessage`, route `CHANNEL_DISCONNECT` | disconnect hook runs, the channel is unregistered, the connection is closed, no response; see [§5.5](#55-reserved-routes-and-connection-lifecycle-hooks) | closing |
| `Channel` | `Once` | `BadRequest` (type mismatch) | `Channel` |

Use one mode per connection and do not send `Once` and `ChannelConnect` concurrently on a fresh connection.

### 5.3. Request/response (Once)

- The client chooses `req_id`; the server returns it unchanged.
- A connection can carry many requests in flight. The only cap is `max_in_flight` for the whole server; there is no per-connection limit.
- **Responses may arrive in a different order** than requests. Correlate by `req_id`.
- If a handler exceeds `request_timeout`, the server answers `Timeout` (11). Cancellation takes effect at the next `.await`; blocking code is not interrupted.
- A handler panic is caught and answered with `InternalError`; the connection stays open.
- Retrying after a timeout or disconnect is up to your application; the protocol gives no idempotency guarantee.

### 5.4. Channel mode

- `ChannelConnect` switches the connection to Channel mode; the server assigns a connection id (`conn_id`) and does not send it to the client.
- The server pushes messages through `ServerHandle`:

  | Method | Waits? | Failure |
  |---|---|---|
  | `send_to_channel(conn_id, Response).await` | yes — until the receiver's queue has room | `Err(String)` |
  | `try_send_to_channel(conn_id, Response)` | no | `Err(String)` if the queue is full or the channel is gone |

  **`conn_id = 0` broadcasts to every channel connection.** Your channel handler receives the `conn_id` of the sender connection. Set the pushed response's `opcode` to `ChannelMessage` (8) yourself.
- Every channel connection has its own queue of **256 messages**. A message to one `conn_id` goes straight to that connection (a hash lookup), not to everybody. An unknown `conn_id` gives `Err("No such channel")`. A broadcast with no channel connections is `Ok`.
- `Ok(())` means "accepted into the receiver's queue", not "delivered". Messages are not dropped silently: if a queue is full, `send_to_channel` waits and `try_send_to_channel` fails.
- **A receiver that does not read blocks broadcasts.** `send_to_channel(0, ..)` visits the connections one by one and waits on a full queue, so one stalled client delays everybody behind it (measured on loopback: with one silent client and 64 KiB messages, another client stopped receiving after about 1300 of 3000). On the first failure `send_to_channel(0, ..)` stops and the remaining connections do not get that message; `try_send_to_channel(0, ..)` tries all and reports `Partial broadcast: N/M channels accepted the message`. For broadcasts to untrusted clients use `try_send_to_channel`, or wrap `send_to_channel` in `tokio::time::timeout`.
- Push messages and regular responses share one outgoing queue; their relative order is not guaranteed.

### 5.5. Reserved routes and connection lifecycle hooks

Two route names carry the channel lifecycle. Both are compared case-insensitively.

| Route | Opcode | Direction | Effect |
|---|---|---|---|
| `CHANNEL_CONNECT` | `ChannelConnect` (1) | client → server | opens Channel mode; the server then runs the **connect hook** with the request (payload and options are available) and the new `conn_id` |
| `CHANNEL_DISCONNECT` | `ChannelMessage` (3) | client → server | runs the **disconnect hook** with the request and the `conn_id`, then the server stops reading from this connection and closes it |

Behaviour:

- Sending `ChannelConnect` with any other route returns `BadRequest`. Clients written for earlier builds that used an arbitrary route must now send `CHANNEL_CONNECT`.
- `CHANNEL_DISCONNECT` is honoured only in `Channel` state; otherwise the answer is `BadRequest` ("Channel not connected").
- Hooks are optional. Without a registered hook the lifecycle route still works, it just runs no user code.
- The hook receives a `Request` copy of the client frame; it returns nothing and cannot reject the connection.
- `CHANNEL_DISCONNECT` produces no response: after the hook the server unregisters the channel and closes the connection. The frame is not routed to channel handlers. A message the hook pushes to the same connection is lost, because the channel is already being closed.
- A panic in the connect hook is caught: the client gets `InternalError`, the channel registration is rolled back and the connection stays `Fresh`. A panic in the disconnect hook is answered with `InternalError` and the connection is not closed.
- The wrong-route answer for `ChannelConnect` is `BadRequest` with the payload `"Route must match the opcode"`. The channel is registered only after the route check.
- A dropped TCP connection (EOF, reset) unregisters the channel but does **not** run the user's disconnect hook. Do not rely on the hook alone to release per-connection state.
- State changes of one connection are decided one frame at a time (a one-permit semaphore per connection). The connect hook runs inside that step, so a slow hook delays the next frames of the same connection.

---

## 6. Errors

| Condition | Response | `req_id` | Connection | Retry? |
|---|---|---|---|---|
| Frame too large / malformed / unknown opcode | `BadRequest` | from header, else 0 | closed | no — fix the frame |
| Wrong version | `BadRequest` | request | open | no |
| Unknown route | `NotFound` | request | open | no |
| Wrong mode for the opcode | `BadRequest` | request | open | no |
| `ChannelConnect` with a route other than `CHANNEL_CONNECT` | `BadRequest` | request | open | no — use the reserved route |
| Second `ChannelConnect` | `Conflict` | request | open | no |
| Handler panic | `InternalError` | request | open | depends on the operation |
| Handler timeout | `Timeout` | request | open | if idempotent |
| TCP error / EOF | none | — | closed | on a new connection |

`Unauthorized` and `Forbidden` are available to your handlers; the library never sends them itself.

---

## 7. Rust API

Public modules: `MMA::config::config` (`Config`, `FramerConfig`, `BigBytesConfig`), `MMA::core::{commands, router, server, handle}` and `MMA::c_api`. The channel table and the TLS acceptor are private. The framer and network layers are private. The crate builds as `rlib`, `cdylib` and `staticlib`.

### 7.1. Types

| Type | Role |
|---|---|
| `Config` | server settings (`Clone`, `Debug`); `Config::default()`, `validate()` |
| `FramerConfig` | wire layout; `FramerConfig::default()`, `validate()` |
| `Request` | `headers`, `payload: Bytes`, `route: String`, `options: Vec<(Bytes, Bytes)>` |
| `Response` | `headers`, `payload: Bytes`, `options`; created with `Response::new(req_id)` |
| `Router` | route table |
| `Server` | owns the listener; `run`, `run_until` |
| `ServerHandle` (`core::handle`) | cheap `Clone`, `Send + Sync`; sends channel messages, changes the configuration |

### 7.2. Configuration

```rust
use MMA::config::config::{Config, FramerConfig};
use std::net::SocketAddr;
use std::time::Duration;

let config = Config {
    address: SocketAddr::from(([0, 0, 0, 0], 8080)),
    frame_config: FramerConfig::default(),
    max_batch: 255,
    max_in_flight: 1024,
    max_connections: 128_000,
    drain_timeout: Duration::from_secs(2),
    shutdown_grace: Duration::from_secs(10),
    request_timeout: Duration::from_secs(10),
    tls_config: None,
};
config.validate().expect("invalid config");
```

| Parameter | Default | Meaning |
|---|---|---|
| `max_connections` | 128 000 | simultaneous connections (≥ 1) |
| `max_in_flight` | 1024 | concurrently running handlers across the server (≥ 1) |
| `max_batch` | 255 (Rust), 16 (C default) | responses written before one flush (≥ 1) |
| `max_message_length` (in `FramerConfig`) | 10 000 000 | maximum body size in bytes |
| `request_timeout` | 10 s | handler time limit |
| `drain_timeout` | 2 s | time to flush pending responses when a connection closes |
| `tls_config` | `None` | optional TLS server configuration (`TlsConfig`) |
| `shutdown_grace` | 10 s | time to wait for running tasks on shutdown before forced cancellation |

`Config::validate` rejects zero limits, an invalid `FramerConfig` and **port 0**. TLS certificate/key loading happens during server initialization, not in `Config::validate`.

#### TLS

MMA can terminate TLS itself (rustls, without client certificates). Set `tls_config` to `Some(TlsConfig { ... })`; use `None` for plain TCP:

```rust
use MMA::config::config::{Config, TlsConfig};
use std::time::Duration;

let config = Config {
    tls_config: Some(TlsConfig {
        tls_certificate_path: "server.crt".into(), // PEM certificate chain
        tls_key_path: "server.key".into(),         // PEM private key
        tls_handshake_timeout: Duration::from_secs(10),
    }),
    ..Config::default()
};
```

- `tls_config: None` disables TLS. `Some(TlsConfig { ... })` enables it and supplies the certificate path, private-key path and handshake timeout together.
- If the certificate or key cannot be loaded, Rust server initialization returns an `io::Error`; the server does not silently fall back to plain TCP. In the C API, startup reports `MMA_SERVER_ERROR`. Check this result and the server log.
- With TLS on, a plain-text client is rejected during the handshake (`TLS Handshake failed`); a handshake that takes longer than `tls_handshake_timeout` is dropped.
- The frame protocol runs unchanged inside the TLS stream. A client must verify the server certificate chain and server name itself. The repository does not provide a CA bundle; configure the trust store appropriate for the certificate you supply.
- TLS settings cannot change while the server runs: `ServerHandle::change_config` returns `InvalidInput` ("TLS settings cannot be changed while server is running") if any TLS field differs.

### 7.3. Handlers

```rust
async fn echo(req: Request, mut res: Response) -> Response {
    res.payload = req.payload;       // Bytes, no copy
    res
}

let mut router = Router::new();
router.register_route("GET", echo)?;
```

A handler takes `Request` and `Response` by value and returns the `Response`. The status defaults to `Ok`; change `res.headers.opcode` or use `Response::bad_request(req_id, Some(payload))`, `Response::conflict(..)`, etc.

Channel handler:

```rust
use MMA::core::commands::ResponseOpcode;

router.register_channel_handler("SEND".to_string(), {
    let handle = handle.clone();                      // ServerHandle
    move |req: Request, _conn_id: u32| {
        let handle = handle.clone();
        async move {
            let mut msg = Response::new(0);
            msg.headers.opcode = ResponseOpcode::ChannelMessage;
            msg.payload = req.payload;
            let _ = handle.send_to_channel(0, msg).await;   // 0 = everyone
        }
    }
})?;
```

Shared state — capture an `Arc` in the closure:

```rust
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

let counter = Arc::new(AtomicU64::new(0));
router.register_route("COUNT", {
    let counter = counter.clone();
    move |_req, mut res: Response| {
        let counter = counter.clone();
        async move {
            res.payload = Bytes::from((counter.fetch_add(1, Ordering::Relaxed) + 1).to_string());
            res
        }
    }
})?;
```

Connection lifecycle hooks (optional, see [§5.5](#55-reserved-routes-and-connection-lifecycle-hooks)):

```rust
router.register_on_channel_connect_handler({
    let handle = handle.clone();
    move |req: Request, conn_id: u32| {
        let handle = handle.clone();
        async move {
            let mut msg = Response::new(0);
            msg.headers.opcode = ResponseOpcode::ChannelMessage;
            msg.payload = Bytes::from(format!("connected: {conn_id}, payload: {} bytes", req.payload.len()));
            let _ = handle.send_to_channel(0, msg).await;
        }
    }
});

router.register_on_channel_disconnect_handler(|req: Request, conn_id: u32| async move {
    // release per-connection state here
});
```

Both calls have the signature `Fn(Request, u32) -> impl Future<Output = ()>` and do not return a `Result`. Registering again replaces the previous hook. The reserved route names are also exported as `router::ReservedRoutes` (`CHANNEL_CONNECT`, `CHANNEL_DISCONNECT`). `Request` now implements `Clone`.

Do not block inside handlers: `request_timeout` cannot cancel blocking code, and it occupies a runtime worker. Use `tokio::task::spawn_blocking` for blocking or CPU-heavy work.

### 7.4. Server lifecycle

| Method | Behaviour |
|---|---|
| `Server::new(config).await` | validates the config and binds the listener |
| `Server::from_listener(config, listener)` | uses an existing listener (the config is still validated, so port 0 is rejected) |
| `server.handle()` | returns a `ServerHandle` |
| `handle.change_config(config)` | validates and publishes a new configuration; TLS fields must stay the same; `Err(io::Error)` otherwise |
| `server.run(router).await` | serves until Ctrl+C, then shuts down gracefully |
| `server.run_until(router, fut).await` | serves until `fut` completes |
| `server.shutdown_token()` / `shutdown_server()` | cancellation token / trigger shutdown |
| `server.local_addr()` | bound address |

Shutdown: accepting stops, readers stop, running handlers get `shutdown_grace`, then everything is cancelled.

### 7.5. Errors

| Call | Error | Cause |
|---|---|---|
| `Server::new` | `io::Error` | invalid config or bind failure |
| `Router::register_route` / `register_channel_handler` | `Err` | empty or too long route, duplicate route |
| `Router::register_on_channel_connect_handler` / `register_on_channel_disconnect_handler` | — | infallible; a new call replaces the old hook |
| `ServerHandle::send_to_channel` / `try_send_to_channel` | `Err(String)` | unknown `conn_id`, closed receiver, full queue (`try_` only), partial broadcast |
| `Config::validate` | `ConfigError` | zero limit, port 0, invalid `FramerConfig` |
| `Server::run` / `run_until` | `io::Error` | listener/accept failure |

---

## 8. C ABI

The public C interface is defined by [`include/mma.h`](include/mma.h). The current API is an **opaque-handle, callback-based API**: C code never allocates or accesses the internals of `MMA_Server`, `MMA_ServerHandle`, `MMA_Request` or `MMA_Response`.

Each `v*` GitHub Release contains native packages for Windows x86-64, Linux x86-64, macOS Intel x86-64 and macOS Apple Silicon arm64. Every package contains the shared library and `mma.h`; the Windows package also contains `MMA.dll.lib`. A SHA-256 file is published beside each archive.

### 8.1. Build and link

Build the library first:

```bash
cargo build --locked --release --lib
```

Linux example (repeat with `example_with_tls.c` to build the TLS version):

```bash
cc -std=c11 -D_POSIX_C_SOURCE=200809L \
    -Wall -Wextra -Wpedantic -Werror \
    -Iinclude examples/example_no_tls.c \
    -Ltarget/release -Wl,-rpath,"$PWD/target/release" \
    -lMMA -pthread -o basic_server

./basic_server
```

On Windows, use the MSVC target and import library produced by Cargo. With LLVM Clang configured for the MSVC toolchain:

```powershell
cargo build --locked --release --lib

clang --target=x86_64-pc-windows-msvc `
    -fuse-ld=link `
    -std=c11 -O2 `
    -Wall -Wextra -Wpedantic -Werror `
    -Iinclude `
    examples/example_no_tls.c `
    target/release/MMA.dll.lib `
    -o basic_server.exe

Copy-Item .\target\release\MMA.dll .\MMA.dll -Force
.\basic_server.exe
```

For TLS, compile `examples/example_with_tls.c` in place of `example_no_tls.c`, then pass the PEM certificate and private-key paths when starting the executable:

```powershell
.\basic_server.exe server.crt server.key
```

The TLS timeout in `MMA_TLSConfig.tls_handshake_timeout` is expressed in **milliseconds** (`10000` means 10 seconds). Keep the matching `MMA.dll` beside the executable or configure the platform's dynamic-library search path. macOS builds use `libMMA.dylib`; configure `@rpath`/`DYLD_LIBRARY_PATH` as appropriate.

### 8.2. API model and server lifecycle

The normal lifecycle is:

1. Call `mma_server_config_default` to initialize a complete configuration.
2. Override the desired fields and, for TLS, set `tls_config` to a pointer to a valid `MMA_TLSConfig`.
3. Call `mma_server_create`.
4. Register routes, channel handlers and optional connect/disconnect hooks. Registration must happen before start.
5. Call `mma_server_start`. It starts the server runtime on a dedicated OS thread and returns only after the listener is ready.
6. Only **after a successful start**, call `mma_server_get_handle` if callbacks need to send channel messages or change configuration. `mma_server_local_port` can also be called after start.
7. On shutdown, call `mma_server_stop`, destroy the handle with `mma_server_handle_destroy`, then destroy the server with `mma_server_destroy`.

The server is a one-shot object: do not start it more than once or call `stop`/`destroy` concurrently on the same pointer. `mma_server_get_handle` fails if the server has not started, its runtime thread has exited, or its internal handle is unavailable.

### 8.3. Opaque requests and callback types

The new callback signatures return `void`:

```c
typedef void (*MMA_RouteCallback)(MMA_Request *request, void *user_data);
typedef void (*MMA_ChannelCallback)(
    MMA_Request *request,
    uint32_t connection_id,
    void *user_data
);
```

- A route callback handles a `Once` request and must eventually complete it with `mma_request_complete`.
- A channel callback handles a channel message or a lifecycle notification. It receives the `connection_id`; it does **not** return a response.
- The connect and disconnect hooks use `MMA_ChannelCallback` and are registered globally, without a route name.
- `user_data` is passed back unchanged. If shared between callbacks, protect mutable state with appropriate synchronization and keep it alive until the server has stopped and callbacks have returned.

Callbacks are invoked **synchronously on Tokio runtime worker threads**. They are not dispatched to a C worker pool or `spawn_blocking`. Keep callbacks short and non-blocking. A blocking C callback occupies a runtime worker; a request timeout cannot forcibly interrupt a C function that is currently executing. For blocking I/O or CPU work, retain the request and enqueue it into an application-owned, bounded worker pool, then return from the callback.

`MMA_Request` is opaque. Inspect it only through the request functions:

- `mma_request_get_info` fills `MMA_RequestInfo` with route, opcode, version, request ID and payload pointer/length.
- `mma_request_option_count` returns the number of options.
- `mma_request_option_at` returns the key/value pointer and explicit length for one option index. Duplicate option keys are allowed by the current wire decoder; enumerate by index rather than assuming key uniqueness.
- `mma_request_is_cancelled` checks whether a retained route request has been cancelled (for example, after timeout or shutdown).

The route and payload pointers in `MMA_RequestInfo`, and the key/value pointers returned by `mma_request_option_at`, are **borrowed**. They remain valid only while the corresponding request reference is alive. Copy bytes that must outlive that reference.

### 8.4. Completing a route request

A callback that can finish immediately may read the request, create a response, and complete the request before returning:

```c
#include "mma.h"
#include <stdio.h>

static void echo_route(MMA_Request *request, void *user_data)
{
    (void)user_data;

    MMA_RequestInfo info;
    if (mma_request_get_info(request, &info) != MMA_OK) {
        return;
    }

    MMA_Response *response = mma_response_new(info.request_id);
    if (response == NULL) {
        return;
    }

    int32_t rc = mma_response_set_status(response, MMA_RESPONSE_OK);
    if (rc == MMA_OK) {
        rc = mma_response_set_payload(response, info.payload, info.payload_len);
    }
    if (rc != MMA_OK) {
        mma_response_destroy(response);
        return;
    }

    rc = mma_request_complete(request, response);
    if (rc == MMA_INVALID_ARGUMENT) {
        /* Ownership was not transferred. */
        mma_response_destroy(response);
    }
    /* On OK, ALREADY_COMPLETED or CANCELLED, response is consumed. */
}
```

`mma_response_set_payload` and `mma_response_add_option` **copy** the supplied buffers. `mma_response_new` creates a library-owned opaque response; do not allocate or modify one directly.

`mma_request_complete` has an explicit ownership rule:

- For a valid route request, the library consumes `response` on `MMA_OK`, `MMA_REQUEST_ALREADY_COMPLETED` and `MMA_REQUEST_CANCELLED`.
- On `MMA_INVALID_ARGUMENT`, ownership remains with the caller; destroy the response if it was allocated.
- A channel/lifecycle notification is not a response-bearing request. Calling `mma_request_complete` for it returns `MMA_INVALID_ARGUMENT`; the caller still owns the response.
- A route must complete only once. A second completion returns `MMA_REQUEST_ALREADY_COMPLETED`.

If response construction fails or the callback returns without completing the request, the request remains pending until the server's `request_timeout` or shutdown policy resolves it. A timeout produces a `TIMEOUT` response; retained route requests can observe cancellation with `mma_request_is_cancelled`.

### 8.5. Deferring work: `retain` / `release`

The callback's request pointer is valid during the callback. To use it after returning, acquire an additional reference **inside the callback**:

```c
static void route_callback(MMA_Request *request, void *user_data)
{
    if (mma_request_retain(request) != MMA_OK) {
        return;
    }

    /* Pseudocode: submit_to_bounded_pool(worker, request, user_data). */
    if (!submit_to_bounded_pool(request, user_data)) {
        mma_request_release(request);
    }
}

static void worker(MMA_Request *request, void *user_data)
{
    /* Read/copy request data, build a response and call complete(). */
    /* ... */
    mma_request_release(request);
}
```

`retain` increments the request's internal reference count; it does not copy the request or its payload. Every successful `mma_request_retain` must be paired with exactly one `mma_request_release`, including queue-submission failures and cancellation paths. Release only after the worker has finished using the request and all borrowed pointers. Do not use the request pointer after releasing your reference.

For retained route requests, check `mma_request_is_cancelled` before expensive work or before attempting a late completion. Cancellation does not automatically stop an application worker; the application must observe it and stop its own work.

### 8.6. Channel messages and server handles

Channel and lifecycle callbacks are notifications. The `MMA_Request` pointer has the same inspection API, but `mma_request_complete` is not valid for these callbacks. Retain the request if it must be processed after the callback returns.

Obtain a handle only after `mma_server_start` succeeds:

```c
MMA_ServerHandle *handle = NULL;
int32_t rc = mma_server_get_handle(server, &handle);
if (rc != MMA_OK || handle == NULL) {
    /* handle acquisition failed */
}
```

Use `mma_server_handle_send_to_channel` to push a message. It uses a **non-waiting** send path. `conn_id == 0` broadcasts to all channel connections; a nonzero ID targets one connection. The function clones the response internally, so the caller may destroy its response after the call. `MMA_SERVER_ERROR` indicates delivery failure (for example, unknown connection, full 256-message queue or partial broadcast). Do not interpret it as a retry-safe delivery guarantee.

Use `mma_server_handle_change_config` to publish a validated config update. The running connections pick up the new frame configuration and request timeout. The bind address/port, `max_batch`, `max_connections`, `max_in_flight` and `shutdown_grace` are not dynamically reconfigured. TLS settings cannot be changed while the server is running. Both handle functions are thread-safe to call from application threads while the handle remains alive.

### 8.7. Functions and status codes

| Function | Purpose |
|---|---|
| `mma_server_config_default`, `mma_server_create` | initialize config and allocate server |
| `mma_server_register_route` | register a Once route |
| `mma_server_register_channel_handler` | register a channel route |
| `mma_server_register_on_channel_connect_handler`, `...disconnect_handler` | register lifecycle hooks |
| `mma_server_start`, `mma_server_local_port`, `mma_server_stop`, `mma_server_destroy` | server lifecycle and bound port |
| `mma_request_retain`, `mma_request_release` | manage request lifetime across callback return |
| `mma_request_get_info`, `mma_request_option_count`, `mma_request_option_at`, `mma_request_is_cancelled` | inspect request and cancellation state |
| `mma_request_complete` | complete a route request with a response |
| `mma_response_new`, `mma_response_set_status`, `mma_response_set_payload`, `mma_response_add_option`, `mma_response_destroy` | construct and free responses |
| `mma_server_get_handle`, `mma_server_handle_destroy` | obtain/free a cloneable server handle |
| `mma_server_handle_send_to_channel`, `mma_server_handle_change_config` | channel push and live config update |

| Code | Name | Meaning |
|---:|---|---|
| 0 | `MMA_OK` | success |
| −1 | `MMA_INVALID_ARGUMENT` | null pointer or invalid argument |
| −2 | `MMA_INVALID_CONFIG` | invalid configuration |
| −3 | `MMA_SERVER_ERROR` | startup/runtime/operation failure |
| −4 | `MMA_SERVER_ALREADY_STARTED` | server already started |
| −5 | `MMA_REQUEST_ALREADY_COMPLETED` | route request already completed (response consumed) |
| −6 | `MMA_REQUEST_CANCELLED` | route request cancelled (response consumed) |

The response opcodes are `MMA_RESPONSE_OK`, `MMA_RESPONSE_BAD_REQUEST`, `MMA_RESPONSE_UNAUTHORIZED`, `MMA_RESPONSE_FORBIDDEN`, `MMA_RESPONSE_NOT_FOUND`, `MMA_RESPONSE_CONFLICT`, `MMA_RESPONSE_INTERNAL_ERROR`, `MMA_RESPONSE_CHANNEL_MESSAGE`, `MMA_RESPONSE_SHUTDOWN`, `MMA_RESPONSE_CHANNEL_CONNECTED` and `MMA_RESPONSE_TIMEOUT` (values 1–11).

### 8.8. Configuration, strings and lifetime

`MMA_ServerConfig` contains `const MMA_TLSConfig *tls_config`; `NULL` disables TLS. The timeout fields (`drain_timeout_ms`, `shutdown_grace_ms`, `request_timeout_ms`, and `tls_handshake_timeout`) are milliseconds. On x86-64, `MMA_ServerConfig` is 72 bytes and `MMA_TLSConfig` is 24 bytes with the current `repr(C)` layout. Always compile against the `mma.h` shipped with the exact native library you link.

The library copies `bind_ip`, certificate/key paths, registered route names, payloads and option buffers at the relevant API call. Keep the config and nested TLS struct valid for the duration of `mma_server_create` or `mma_server_handle_change_config`; their strings may be released after the call returns. `user_data` is not copied: keep it alive until callbacks have stopped. For `mma_response_set_payload` and `mma_response_add_option`, source buffers may be freed after the call returns.

- Ignore `SIGPIPE` in a POSIX C host before starting the server (`signal(SIGPIPE, SIG_IGN)`). The example programs should be used as reference for signal handling. On Windows, console control events are handled through the Windows console API.
- Keep the `MMA_ServerHandle` alive while using it; destroy it before destroying the owning server. `mma_server_handle_destroy` does not stop the server.
- Never call `mma_server_stop` or `mma_server_destroy` from a callback of that same server; `stop` joins the runtime thread.
- The current C ABI is not ABI-compatible with the pre-async C header from the previous release snapshot. Recompile the C application with the current header and link it with the matching library; see [§9](#9-compatibility-and-versioning).

The complete implementations are [`examples/example_no_tls.c`](examples/example_no_tls.c) and [`examples/example_with_tls.c`](examples/example_with_tls.c). They demonstrate `POW`, `ECHO` and `CHANGE` routes, `SEND` channel handling, connection lifecycle callbacks, response ownership, server-handle management and clean shutdown.

---

## 9. Compatibility and versioning

Three versions are independent:

| What | Now | Rule |
|---|---|---|
| Wire protocol | `MMA_VERSION = 1` | bumped on incompatible frame or semantics changes |
| Rust crate | `0.1.0` in `Cargo.toml` (git tags go up to `v0.1.4`) | before 1.0, breaking changes may occur in any release; module paths and the push API changed (see [CHANGELOG.md](CHANGELOG.md)) |
| C ABI / `mma.h` | unversioned | Breaking since the previous release snapshot: `MMA_Request` and `MMA_Response` became opaque handles; callback signatures changed to `void` and route responses use `mma_request_complete`. Recompile all C consumers against the current `mma.h`; there is no runtime ABI version query |

Two peers are wire-compatible only if `MMA_VERSION` **and** the whole `FramerConfig` match. Note: the reserved-route rule for `ChannelConnect` ([§5.5](#55-reserved-routes-and-connection-lifecycle-hooks)) changes request semantics without changing the frame layout or `MMA_VERSION`; see [CHANGELOG.md](CHANGELOG.md). Adding opcodes or a mandatory field requires a new `MMA_VERSION`; an old server closes the connection on an unknown request opcode.

---

## 10. Security

- **TLS is optional and server-side only** (see [§7.2](#72-configuration)); there is no client authentication, no application-level authentication and no replay protection. Without TLS everything, including the layout, is readable on the wire. The layout configuration is obfuscation and is not a security mechanism. If the configured certificate or key cannot be loaded, server initialization fails; it does not silently fall back to plain TCP.
- The repository does not ship certificates or private keys. Supply a certificate and matching private key when running a TLS example. Use dedicated test credentials for local testing; never commit or distribute production private keys.
- Treat every field from the client as untrusted. Only `route` is UTF-8-validated; payload and options are raw bytes.

| Limit | Value |
|---|---|
| Frame body | `max_message_length` (default 10 000 000), checked before allocation |
| Route | ≤ 255 bytes |
| Options | ≤ 255 per frame; key and value 1–65535 bytes |
| Connections | `max_connections` |
| In-flight requests | `max_in_flight` globally; no per-connection cap |
| Channel push queue | 256 messages per channel connection |
| Handler time | `request_timeout` for async request handling; a synchronous C callback itself cannot be interrupted |

Not covered yet: idle/read/write timeouts (slow-client attacks), a global byte budget for in-flight data, and protection against clients that do not read responses (they also stall `send_to_channel(0, ..)`, see [§5.4](#54-channel-mode)).

---

## 11. Repository layout

| Path | Contents |
|---|---|
| `src/protocol/framer.rs` | frame encoder/decoder (private) |
| `src/network/network.rs` | per-connection loop, state machine (private) |
| `src/config/config.rs` | `Config`, `FramerConfig`, `BigBytesConfig`, `MMA_VERSION` |
| `src/core/` | `commands` (Request/Response, `SocketStatus`), `router` (routes, handlers, lifecycle hooks), `server`, `handle` (`ServerHandle`), `channel_hub` and `tls` (private) |
| `src/c_api.rs` | C ABI exports (opaque handles, request reference counting and asynchronous route completion) |
| `include/mma.h` | C header |
| `.gitattributes` | normalizes text files to LF to keep diffs stable across operating systems |
| `examples/example_no_tls.rs`, `examples/example_with_tls.rs` | Rust server examples without and with TLS; both demonstrate route/channel handlers and connection lifecycle hooks |
| `examples/example_no_tls.c`, `examples/example_with_tls.c` | C ABI server examples without and with TLS; the TLS example accepts certificate and private-key paths |
| `mma_gui_client.py` | Tkinter test client (Python standard library): configurable layout, all three opcodes, a codec self-test |
| `.github/workflows/native-release.yml` | CI on pushes, pull requests and manual dispatch: Rust tests, release library and example builds, both C examples compiled on Linux and Windows/MSVC, and C/C++ public-header checks. On `v*` tags it builds four native packages (Windows x86-64, Linux x86-64, macOS x86-64 and macOS arm64), verifies SHA-256 files and publishes a GitHub Release |
| `CHANGELOG.md`, `CHANGELOG.ru.md` | English and Russian history of changes with wire / Rust API / C ABI impact |
| `tests/` | configuration tests (codec tests are currently disabled) |