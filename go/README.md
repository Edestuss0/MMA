# MMA Go bindings (`cabi`)

The `cabi` package preserves the public Go API from MMA-MRC while using the
current opaque-pointer C ABI from `include/mma.h`. It uses `purego`; CGO and a
separate `mma_go_bridge` library are not required.

## Native library

The package loads exactly one native library:

- Windows amd64: `MMA.dll`
- Linux amd64: `libMMA.so`
- macOS amd64/arm64: `libMMA.dylib`

Set `MMA_NATIVE_DIR` to the directory containing that library to use a local
build. Otherwise the package downloads the platform archive matching
`MMA_NATIVE_VERSION` or the Go module version and verifies its SHA-256 file.

PowerShell example:

```powershell
$env:MMA_NATIVE_DIR = 'E:\projects\messanger\MMA\target\release'
go test ./...
```

## Existing API remains available

`Request`, `Response`, `ServerConfig`, `RouteHandler`, `ChannelHandler`,
`ResponseBuilder`, `NewServer`, `RegisterRoute`, `Handle`,
`RegisterChannelHandler`, `Start`, `BoundPort`, `GetHandle`, `Stop`, `Close`,
`SendToChannel`, and `ChangeConfig` keep their existing public signatures.

`NewServerWithTLS`, `RegisterOnChannelConnectHandler`,
`RegisterOnChannelDisconnectHandler` are additive. They do not change the layout
of the existing `ServerConfig` type.

TLS settings are fixed for the lifetime of a running server. `ChangeConfig`
preserves the TLS configuration used when the server was created; the native
engine rejects attempts to change TLS settings while it is running.

## Callback and ownership model

Native callbacks are invoked synchronously by the Rust/Tokio runtime. The
package callbacks return quickly and dispatch user handlers in Go goroutines.
For route callbacks, the binding calls `mma_request_retain` before returning
to Rust, copies request data in a Go goroutine, completes the request, and
then calls `mma_request_release`. This allows route work to continue without
occupying a Tokio worker.

Channel and lifecycle callbacks copy request data and invoke `ChannelHandler`
synchronously before returning to Rust. This preserves the order of messages
on each TCP connection. Keep these handlers short and non-blocking; enqueue
the copied `Request` into an application-owned Go worker when longer work is
required. They do not need `retain/release` because no native request pointer
escapes the callback.

Request metadata, route, payload, option keys, and option values are copied
into Go-owned memory. Go applications never reinterpret the opaque
`MMA_Request` pointer as a Go struct.

Route handlers construct a native response through `ResponseBuilder`. The
binding completes the request with `mma_request_complete`. The C ABI consumes
response ownership for every route-completion result except
`MMA_INVALID_ARGUMENT`; in that one case the binding destroys the response.
`MMA_REQUEST_ALREADY_COMPLETED` and `MMA_REQUEST_CANCELLED` still consume it.

Channel and lifecycle handlers are notifications. They do not create or
complete responses. Errors returned by `ChannelHandler` and handler panics are
written to the process log because the native notification callback has no
response channel.

The native request timeout does not forcibly terminate a Go handler. A handler
that finishes after timeout receives `MMA_REQUEST_CANCELLED` when its response
is submitted. Its retained request is still released normally.

## TLS

TLS is optional and additive:

```go
server, err := cabi.NewServerWithTLS(config, cabi.TLSConfig{
    CertificatePath:  "server.crt",
    KeyPath:          "server.key",
    HandshakeTimeout: 10 * time.Second,
})
```

The existing `NewServer(config)` call continues to create a non-TLS server.

## Tests

```powershell
go test ./...
go vet ./...
$env:CGO_ENABLED = '0'
go test ./...
```

Native ABI defaults can be checked with:

```powershell
$env:MMA_NATIVE_DIR = 'D:\path\to\native\release'
go test -tags native_integration ./cabi
```
