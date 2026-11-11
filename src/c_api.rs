#![allow(non_camel_case_types)]

use crate::config::config::{BigBytesConfig, Config, FramerConfig, TlsConfig};
use crate::core::commands::{Request, Response, ResponseOpcode};
use crate::core::handle::ServerHandle;
use crate::core::router::Router;
use crate::core::server::Server;
use bytes::Bytes;
use std::ffi::{CStr, c_char, c_void};
use std::net::{IpAddr, SocketAddr};
use std::ptr;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub const MMA_OK: i32 = 0;
pub const MMA_INVALID_ARGUMENT: i32 = -1;
pub const MMA_INVALID_CONFIG: i32 = -2;
pub const MMA_SERVER_ERROR: i32 = -3;
pub const MMA_SERVER_ALREADY_STARTED: i32 = -4;

/// Internal safety limit for simultaneously running synchronous C callbacks.
/// This does not affect the public C ABI.
const MMA_MAX_CONCURRENT_C_CALLBACKS: usize = 128;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MMA_FramerConfig {
    pub max_message_length: u32,
    pub opcode_pos: u8,
    pub version_pos: u8,
    pub route_len_pos: u8,
    pub route_order: u8,
    pub payload_order: u8,
    pub options_order: u8,
    pub options_count_pos: u8,
    pub options_key_first: u8,
    pub header_length: u8,
    pub big_bytes_config: MMA_BigBytesConfig,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MMA_BigBytesConfig {
    pub options_len_order: u8,
    pub payload_len_order: u8,
    pub req_id_order: u8,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MMA_TLSConfig {
    pub tls_certificate_path: *const c_char,
    pub tls_key_path: *const c_char,
    pub tls_handshake_timeout: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MMA_ServerConfig {
    pub bind_ip: *const c_char,
    pub port: u16,
    pub max_batch: u32,
    pub max_in_flight: u32,
    pub framer: MMA_FramerConfig,
    pub max_connections: u32,
    /// Graceful connection drain timeout, in milliseconds.
    pub drain_timeout_ms: u64,
    /// Maximum time allowed for server shutdown, in milliseconds.
    pub shutdown_grace_ms: u64,
    /// Maximum time allowed for handling one request, in milliseconds.
    pub request_timeout_ms: u64,
    pub tls_config: *const MMA_TLSConfig,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MMA_Option {
    pub key: *const u8,
    pub key_len: usize,
    pub value: *const u8,
    pub value_len: usize,
}

#[repr(C)]
pub struct MMA_Request {
    pub route: *const u8,
    pub route_len: usize,
    pub opcode: u8,
    pub version: u8,
    pub request_id: u32,
    pub payload: *const u8,
    pub payload_len: usize,
    pub options: *const MMA_Option,
    pub options_count: usize,
}

pub struct MMA_ServerHandle {
    inner: ServerHandle,
}

#[repr(C)]
pub struct MMA_Response {
    response: *mut Response,
}

// These signatures are the existing public ABI. They remain synchronous C callbacks.
pub type MMA_RouteCallback =
    Option<extern "C" fn(MMA_Request, MMA_Response, *const c_void) -> MMA_Response>;
pub type MMA_ChannelHandlerCallback =
    Option<extern "C" fn(*const MMA_Request, u32, *const c_void) -> i32>;

struct RegisteredRoute {
    route: String,
    callback: extern "C" fn(MMA_Request, MMA_Response, *const c_void) -> MMA_Response,
    user_data: usize,
}

struct RegisteredChannelHandler {
    route: String,
    callback: extern "C" fn(*const MMA_Request, u32, *const c_void) -> i32,
    user_data: usize,
}

/// A connection lifecycle callback has the same C signature as a channel
/// handler, but it is registered globally rather than under a route name.
struct RegisteredLifecycleHandler {
    callback: extern "C" fn(*const MMA_Request, u32, *const c_void) -> i32,
    user_data: usize,
}

pub struct MMA_Server {
    config: Config,
    bound_port: u16,
    routes: Vec<RegisteredRoute>,
    channel_handlers: Vec<RegisteredChannelHandler>,
    on_channel_connect_handler: Option<RegisteredLifecycleHandler>,
    on_channel_disconnect_handler: Option<RegisteredLifecycleHandler>,
    started: bool,
    stop: CancellationToken,
    thread: Option<JoinHandle<()>>,
    handle: Option<ServerHandle>,
}

fn framer_config_from_c(config: MMA_FramerConfig) -> Option<FramerConfig> {
    if config.options_key_first > 1 {
        return None;
    }

    Some(FramerConfig {
        max_message_length: config.max_message_length,
        opcode_pos: config.opcode_pos,
        version_pos: config.version_pos,
        route_len_pos: config.route_len_pos,
        route_order: config.route_order,
        payload_order: config.payload_order,
        options_order: config.options_order,
        options_count_pos: config.options_count_pos,
        options_key_first: config.options_key_first != 0,
        header_length: config.header_length,
        big_bytes_config: BigBytesConfig {
            options_len_order: config.big_bytes_config.options_len_order,
            payload_len_order: config.big_bytes_config.payload_len_order,
            req_id_order: config.big_bytes_config.req_id_order,
        },
    })
}

fn framer_config_to_c(config: &FramerConfig) -> MMA_FramerConfig {
    MMA_FramerConfig {
        max_message_length: config.max_message_length,
        opcode_pos: config.opcode_pos,
        version_pos: config.version_pos,
        route_len_pos: config.route_len_pos,
        route_order: config.route_order,
        payload_order: config.payload_order,
        options_order: config.options_order,
        options_count_pos: config.options_count_pos,
        options_key_first: u8::from(config.options_key_first),
        header_length: config.header_length,
        big_bytes_config: MMA_BigBytesConfig {
            options_len_order: config.big_bytes_config.options_len_order,
            payload_len_order: config.big_bytes_config.payload_len_order,
            req_id_order: config.big_bytes_config.req_id_order,
        },
    }
}

fn config_from_c(config: &MMA_ServerConfig) -> Option<Config> {
    if config.bind_ip.is_null() {
        return None;
    }

    let ip_str = unsafe { CStr::from_ptr(config.bind_ip) }.to_str().ok()?;
    let ip_addr = IpAddr::from_str(ip_str).ok()?;

    let tls_config = if config.tls_config.is_null() {
        None
    } else {
        let tls_sub = unsafe { &*config.tls_config };

        let cert_path = if tls_sub.tls_certificate_path.is_null() {
            None
        } else {
            let c_str = unsafe { CStr::from_ptr(tls_sub.tls_certificate_path) };
            c_str.to_str().map(|s| s.to_owned()).ok()
        };

        let key_path = if tls_sub.tls_key_path.is_null() {
            None
        } else {
            let c_str = unsafe { CStr::from_ptr(tls_sub.tls_key_path) };
            c_str.to_str().map(|s| s.to_owned()).ok()
        };

        if key_path.is_none() || cert_path.is_none() {
            return None;
        }

        Some(TlsConfig {
            tls_certificate_path: cert_path.unwrap(),
            tls_key_path: key_path.unwrap(),
            tls_handshake_timeout: Duration::from_millis(tls_sub.tls_handshake_timeout),
        })
    };

    Some(Config {
        address: SocketAddr::new(ip_addr, config.port),
        frame_config: framer_config_from_c(config.framer)?,
        max_batch: config.max_batch,
        max_connections: config.max_connections,
        max_in_flight: config.max_in_flight,
        drain_timeout: Duration::from_millis(config.drain_timeout_ms),
        shutdown_grace: Duration::from_millis(config.shutdown_grace_ms),
        request_timeout: Duration::from_millis(config.request_timeout_ms),
        tls_config,
    })
}

fn valid_slice<T>(data: *const T, len: usize) -> bool {
    len == 0 || (!data.is_null() && len <= isize::MAX as usize)
}

unsafe fn raw_slice<'a, T>(data: *const T, len: usize) -> &'a [T] {
    if len == 0 {
        &[]
    } else {
        // SAFETY: the caller must guarantee that data points to len readable,
        // properly aligned elements for the duration of the returned borrow.
        unsafe { std::slice::from_raw_parts(data, len) }
    }
}

/// Invoke a route callback on a Tokio blocking-pool thread.
///
/// The request and all pointers exposed through MMA_Request remain alive only
/// until the synchronous C callback returns. A C callback must not retain them.
fn invoke_c_route(
    callback: extern "C" fn(MMA_Request, MMA_Response, *const c_void) -> MMA_Response,
    request: Request,
    response: Response,
    route_name: String,
    user_data: usize,
) -> Response {
    let request_id = request.headers.req_id;

    let options: Vec<MMA_Option> = request
        .options
        .iter()
        .map(|(key, value)| MMA_Option {
            key: key.as_ptr(),
            key_len: key.len(),
            value: value.as_ptr(),
            value_len: value.len(),
        })
        .collect();

    let request_view = MMA_Request {
        route: route_name.as_ptr(),
        route_len: route_name.len(),
        opcode: request.headers.opcode as u8,
        version: request.headers.version,
        request_id,
        payload: request.payload.as_ptr(),
        payload_len: request.payload.len(),
        options: if options.is_empty() {
            ptr::null()
        } else {
            options.as_ptr()
        },
        options_count: options.len(),
    };

    let response_ptr = Box::into_raw(Box::new(response));
    let response_view = MMA_Response {
        response: response_ptr,
    };

    // The callback is intentionally synchronous, but runs on spawn_blocking's
    // pool rather than on a Tokio async worker thread.
    let returned = callback(request_view, response_view, user_data as *const c_void);
    let returned_ptr = returned.response;

    if returned_ptr.is_null() {
        // SAFETY: response_ptr was created by Box::into_raw above and has not
        // been reclaimed yet. The callback contract forbids destroying it.
        unsafe { drop(Box::from_raw(response_ptr)) };
        return Response::internal_error(request_id);
    }

    if returned_ptr != response_ptr {
        // The callback returned a replacement response (normally created by
        // mma_response_new). Reclaim the initial response that it replaced.
        // SAFETY: response_ptr was allocated above and is still owned here.
        unsafe { drop(Box::from_raw(response_ptr)) };
    }

    // SAFETY: the callback contract requires returned_ptr to be either the
    // original response pointer or a live pointer returned by mma_response_new.
    unsafe { *Box::from_raw(returned_ptr) }
}

/// Invoke a channel callback on a Tokio blocking-pool thread.
///
/// The MMA_Request view and its backing buffers are valid only for this call.
fn invoke_c_channel(
    callback: extern "C" fn(*const MMA_Request, u32, *const c_void) -> i32,
    request: Request,
    conn_id: u32,
    user_data: usize,
) -> i32 {
    let options: Vec<MMA_Option> = request
        .options
        .iter()
        .map(|(key, value)| MMA_Option {
            key: key.as_ptr(),
            key_len: key.len(),
            value: value.as_ptr(),
            value_len: value.len(),
        })
        .collect();

    // The route view points into the owned Request, which remains alive until
    // the synchronous callback returns.
    let request_view = MMA_Request {
        route: request.route.as_ptr(),
        route_len: request.route.len(),
        opcode: request.headers.opcode as u8,
        version: request.headers.version,
        request_id: request.headers.req_id,
        payload: request.payload.as_ptr(),
        payload_len: request.payload.len(),
        options: if options.is_empty() {
            ptr::null()
        } else {
            options.as_ptr()
        },
        options_count: options.len(),
    };

    callback(&request_view, conn_id, user_data as *const c_void)
}

/// Runs a synchronous C channel/lifecycle callback away from Tokio worker
/// threads. The semaphore permit is moved into the blocking closure so a
/// timed-out async waiter cannot release capacity while the C callback is
/// still executing.
async fn run_c_channel_callback(
    callback: extern "C" fn(*const MMA_Request, u32, *const c_void) -> i32,
    request: Request,
    conn_id: u32,
    user_data: usize,
    callback_semaphore: Arc<Semaphore>,
    callback_kind: &'static str,
) {
    let permit = match callback_semaphore.acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => {
            eprintln!("MMA {callback_kind} callback semaphore is closed");
            return;
        }
    };

    let join = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        invoke_c_channel(callback, request, conn_id, user_data)
    });

    match join.await {
        Ok(MMA_OK) => {}
        Ok(status) => {
            eprintln!("MMA {callback_kind} callback returned status {status}");
        }
        Err(error) => {
            eprintln!("MMA {callback_kind} callback task failed: {error}");
        }
    }
}

fn router_from_registrations(
    routes: &[RegisteredRoute],
    channel_handlers: &[RegisteredChannelHandler],
    on_channel_connect_handler: Option<&RegisteredLifecycleHandler>,
    on_channel_disconnect_handler: Option<&RegisteredLifecycleHandler>,
    c_callback_semaphore: Arc<Semaphore>,
) -> Result<Router, String> {
    let mut router = Router::new();

    for route in routes {
        let callback = route.callback;
        let route_name = route.route.clone();
        let user_data = route.user_data;
        let callback_semaphore = Arc::clone(&c_callback_semaphore);

        router
            .register_route(
                route.route.clone(),
                move |request: Request, response: Response| {
                    let route_name = route_name.clone();
                    let callback_semaphore = Arc::clone(&callback_semaphore);

                    async move {
                        let request_id = request.headers.req_id;

                        // Waiting for a slot is async and therefore does not block
                        // a Tokio worker. The request timeout includes this wait.
                        let permit = match callback_semaphore.acquire_owned().await {
                            Ok(permit) => permit,
                            Err(_) => return Response::internal_error(request_id),
                        };

                        let join = tokio::task::spawn_blocking(move || {
                            // Keep the permit until the C function really returns.
                            // If the request times out, the blocking callback keeps
                            // running and continues to occupy this slot.
                            let _permit = permit;
                            invoke_c_route(callback, request, response, route_name, user_data)
                        });

                        match join.await {
                            Ok(response) => response,
                            Err(error) => {
                                eprintln!("MMA C route callback task failed: {error}");
                                Response::internal_error(request_id)
                            }
                        }
                    }
                },
            )
            .map_err(|error| format!("route {:?}: {error}", route.route))?;
    }

    for handler in channel_handlers {
        let callback = handler.callback;
        let user_data = handler.user_data;
        let callback_semaphore = Arc::clone(&c_callback_semaphore);

        router
            .register_channel_handler(
                handler.route.clone(),
                move |request: Request, conn_id: u32| {
                    let callback_semaphore = Arc::clone(&callback_semaphore);
                    async move {
                        run_c_channel_callback(
                            callback,
                            request,
                            conn_id,
                            user_data,
                            callback_semaphore,
                            "channel handler",
                        )
                        .await;
                    }
                },
            )
            .map_err(|error| format!("channel route {:?}: {error}", handler.route))?;
    }

    if let Some(handler) = on_channel_connect_handler {
        let callback = handler.callback;
        let user_data = handler.user_data;
        let callback_semaphore = Arc::clone(&c_callback_semaphore);

        router.register_on_channel_connect_handler(move |request: Request, conn_id: u32| {
            let callback_semaphore = Arc::clone(&callback_semaphore);
            async move {
                run_c_channel_callback(
                    callback,
                    request,
                    conn_id,
                    user_data,
                    callback_semaphore,
                    "on_channel_connect",
                )
                .await;
            }
        });
    }

    if let Some(handler) = on_channel_disconnect_handler {
        let callback = handler.callback;
        let user_data = handler.user_data;
        let callback_semaphore = Arc::clone(&c_callback_semaphore);

        router.register_on_channel_disconnect_handler(move |request: Request, conn_id: u32| {
            let callback_semaphore = Arc::clone(&callback_semaphore);
            async move {
                run_c_channel_callback(
                    callback,
                    request,
                    conn_id,
                    user_data,
                    callback_semaphore,
                    "on_channel_disconnect",
                )
                .await;
            }
        });
    }

    Ok(router)
}

unsafe fn response_mut<'a>(response: MMA_Response) -> Result<&'a mut Response, i32> {
    if response.response.is_null() {
        return Err(MMA_INVALID_ARGUMENT);
    }

    // SAFETY: the caller of the exported mutator must pass a live response
    // handle created by this library and must ensure exclusive access.
    Ok(unsafe { &mut *response.response })
}

fn response_error() -> MMA_Response {
    MMA_Response {
        response: ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_config_default(output: *mut MMA_ServerConfig) -> i32 {
    if output.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    let framer = FramerConfig::default();
    let config = MMA_ServerConfig {
        bind_ip: c"0.0.0.0".as_ptr(),
        port: 8080,
        max_batch: 16,
        max_in_flight: 1024,
        framer: framer_config_to_c(&framer),
        max_connections: 128_000,
        drain_timeout_ms: 2_000,
        shutdown_grace_ms: 10_000,
        request_timeout_ms: 10_000,
        tls_config: ptr::null(),
    };

    // SAFETY: output was checked for null; caller must provide writable storage.
    unsafe { *output = config };
    MMA_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_create(
    config: *const MMA_ServerConfig,
    output: *mut *mut MMA_Server,
) -> i32 {
    if output.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: output was checked for null and must be writable by the caller.
    unsafe { *output = ptr::null_mut() };

    if config.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: config must point to a valid, initialized MMA_ServerConfig.
    let config = unsafe { *config };
    if config.bind_ip.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: bind_ip must be a valid NUL-terminated string.
    let bind_ip = match unsafe { CStr::from_ptr(config.bind_ip) }.to_str() {
        Ok(value) => value,
        Err(_) => return MMA_INVALID_ARGUMENT,
    };

    let ip = match bind_ip.parse::<IpAddr>() {
        Ok(value) => value,
        Err(_) => return MMA_INVALID_ARGUMENT,
    };

    let Some(rust_config) = config_from_c(&config) else {
        return MMA_INVALID_CONFIG;
    };

    if rust_config.frame_config.validate().is_err() || rust_config.validate().is_err() {
        return MMA_INVALID_CONFIG;
    }

    let address = SocketAddr::new(ip, config.port);
    let server = MMA_Server {
        config: rust_config,
        bound_port: address.port(),
        routes: Vec::new(),
        channel_handlers: Vec::new(),
        on_channel_connect_handler: None,
        on_channel_disconnect_handler: None,
        started: false,
        stop: CancellationToken::new(),
        thread: None,
        handle: None,
    };

    // SAFETY: output is writable and receives ownership of the boxed server.
    unsafe { *output = Box::into_raw(Box::new(server)) };
    MMA_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_register_route(
    server: *mut MMA_Server,
    route: *const u8,
    route_len: usize,
    callback: MMA_RouteCallback,
    user_data: *mut c_void,
) -> i32 {
    if server.is_null() || !valid_slice(route, route_len) {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: server must be a live handle exclusively accessible during registration.
    let server = unsafe { &mut *server };
    if server.started {
        return MMA_SERVER_ALREADY_STARTED;
    }

    if route_len == 0 || route_len > u8::MAX as usize {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: route/route_len were validated; caller supplies readable bytes.
    let route = match std::str::from_utf8(unsafe { raw_slice(route, route_len) }) {
        Ok(value) => value,
        Err(_) => return MMA_INVALID_ARGUMENT,
    };

    let Some(callback) = callback else {
        return MMA_INVALID_ARGUMENT;
    };

    server.routes.push(RegisteredRoute {
        route: route.to_owned(),
        callback,
        user_data: user_data as usize,
    });

    MMA_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_register_channel_handler(
    server: *mut MMA_Server,
    route: *const u8,
    route_len: usize,
    callback: MMA_ChannelHandlerCallback,
    user_data: *const c_void,
) -> i32 {
    if server.is_null() || !valid_slice(route, route_len) {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: server must be a live handle exclusively accessible during registration.
    let server = unsafe { &mut *server };
    if server.started || route_len == 0 || route_len > u8::MAX as usize {
        return if server.started {
            MMA_SERVER_ALREADY_STARTED
        } else {
            MMA_INVALID_ARGUMENT
        };
    }

    // SAFETY: route/route_len were validated; caller supplies readable bytes.
    let route = match std::str::from_utf8(unsafe { raw_slice(route, route_len) }) {
        Ok(value) => value,
        Err(_) => return MMA_INVALID_ARGUMENT,
    };

    let Some(callback) = callback else {
        return MMA_INVALID_ARGUMENT;
    };

    server.channel_handlers.push(RegisteredChannelHandler {
        route: route.to_owned(),
        callback,
        user_data: user_data as usize,
    });

    MMA_OK
}

/// Registers the global callback invoked by Router::on_channel_connect.
/// Calling this again before server start replaces the previous callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_register_on_channel_connect_handler(
    server: *mut MMA_Server,
    callback: MMA_ChannelHandlerCallback,
    user_data: *const c_void,
) -> i32 {
    if server.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: server must be a live handle exclusively accessible during registration.
    let server = unsafe { &mut *server };
    if server.started {
        return MMA_SERVER_ALREADY_STARTED;
    }

    let Some(callback) = callback else {
        return MMA_INVALID_ARGUMENT;
    };

    server.on_channel_connect_handler = Some(RegisteredLifecycleHandler {
        callback,
        user_data: user_data as usize,
    });

    MMA_OK
}

/// Registers the global callback invoked by Router::on_channel_disconnect.
/// Calling this again before server start replaces the previous callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_register_on_channel_disconnect_handler(
    server: *mut MMA_Server,
    callback: MMA_ChannelHandlerCallback,
    user_data: *const c_void,
) -> i32 {
    if server.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: server must be a live handle exclusively accessible during registration.
    let server = unsafe { &mut *server };
    if server.started {
        return MMA_SERVER_ALREADY_STARTED;
    }

    let Some(callback) = callback else {
        return MMA_INVALID_ARGUMENT;
    };

    server.on_channel_disconnect_handler = Some(RegisteredLifecycleHandler {
        callback,
        user_data: user_data as usize,
    });

    MMA_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_route(
    server: *mut MMA_Server,
    route: *const u8,
    route_len: usize,
    callback: MMA_RouteCallback,
    user_data: *mut c_void,
) -> i32 {
    // SAFETY: forwards the same argument contract to the canonical function.
    unsafe { mma_server_register_route(server, route, route_len, callback, user_data) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_start(server: *mut MMA_Server) -> i32 {
    if server.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: server must be a live handle exclusively accessible during start.
    let server = unsafe { &mut *server };

    if server.started {
        return MMA_SERVER_ALREADY_STARTED;
    }

    let config = server.config.clone();

    // Bound the number of C callbacks running simultaneously. This semaphore is
    // shared by both route and channel callbacks and does not change the ABI.
    let c_callback_limit =
        (config.max_in_flight.max(1) as usize).min(MMA_MAX_CONCURRENT_C_CALLBACKS);
    let c_callback_semaphore = Arc::new(Semaphore::new(c_callback_limit));

    let router = match router_from_registrations(
        &server.routes,
        &server.channel_handlers,
        server.on_channel_connect_handler.as_ref(),
        server.on_channel_disconnect_handler.as_ref(),
        c_callback_semaphore,
    ) {
        Ok(router) => router,
        Err(error) => {
            eprintln!("MMA router initialization failed: {error}");
            return MMA_SERVER_ERROR;
        }
    };

    let stop = server.stop.clone();
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(SocketAddr, ServerHandle), String>>(1);

    let thread = match thread::Builder::new()
        .name("mma-server".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .max_blocking_threads(c_callback_limit)
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ =
                        ready_tx.send(Err(format!("Tokio runtime initialization failed: {error}")));
                    return;
                }
            };

            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::bind(config.address).await {
                    Ok(listener) => listener,
                    Err(error) => {
                        let _ = ready_tx.send(Err(format!("TCP bind failed: {error}")));
                        return;
                    }
                };

                let local_addr = match listener.local_addr() {
                    Ok(addr) => addr,
                    Err(error) => {
                        let _ =
                            ready_tx.send(Err(format!("Cannot obtain listener address: {error}")));
                        return;
                    }
                };

                let mut running_server = match Server::from_listener(config, listener) {
                    Ok(server) => server,
                    Err(error) => {
                        let _ =
                            ready_tx.send(Err(format!("Server initialization failed: {error}")));
                        return;
                    }
                };

                let handle = running_server.handle();
                if ready_tx.send(Ok((local_addr, handle))).is_err() {
                    return;
                }

                if let Err(error) = running_server.run_until(router, stop.cancelled()).await {
                    eprintln!("MMA server stopped after an error: {error}");
                }
            });

            // Dropping runtime waits for already-started spawn_blocking jobs.
            // Therefore mma_server_stop may wait past shutdown_grace_ms if a C
            // callback is still executing. A synchronous C function cannot be
            // forcibly interrupted while preserving this ABI.
        }) {
        Ok(thread) => thread,
        Err(error) => {
            eprintln!("Failed to create MMA server thread: {error}");
            return MMA_SERVER_ERROR;
        }
    };

    match ready_rx.recv() {
        Ok(Ok((local_addr, handle))) => {
            server.bound_port = local_addr.port();
            server.handle = Some(handle);
            server.thread = Some(thread);
            server.started = true;
            MMA_OK
        }
        Ok(Err(error)) => {
            eprintln!("MMA server startup failed: {error}");
            let _ = thread.join();
            MMA_SERVER_ERROR
        }
        Err(error) => {
            eprintln!("MMA server startup handshake failed: {error}");
            let _ = thread.join();
            MMA_SERVER_ERROR
        }
    }
}

/// Creates an independent C handle from a running server.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_get_handle(
    server: *const MMA_Server,
    output: *mut *mut MMA_ServerHandle,
) -> i32 {
    if server.is_null() || output.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: output must be writable by the caller.
    unsafe { *output = ptr::null_mut() };

    // SAFETY: server must point to a live MMA_Server.
    let server = unsafe { &*server };

    if !server.started {
        return MMA_SERVER_ERROR;
    }

    let Some(thread) = server.thread.as_ref() else {
        return MMA_SERVER_ERROR;
    };

    if thread.is_finished() {
        return MMA_SERVER_ERROR;
    }

    let Some(handle) = server.handle.as_ref() else {
        return MMA_SERVER_ERROR;
    };

    let handle = Box::new(MMA_ServerHandle {
        inner: handle.clone(),
    });

    // SAFETY: output is writable and receives ownership of the boxed handle.
    unsafe { *output = Box::into_raw(handle) };
    MMA_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_handle_destroy(handle: *mut MMA_ServerHandle) {
    if !handle.is_null() {
        // SAFETY: handle must be a live, uniquely-owned pointer from get_handle.
        unsafe { drop(Box::from_raw(handle)) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_handle_send_to_channel(
    handle: *const MMA_ServerHandle,
    conn_id: u32,
    message: MMA_Response,
) -> i32 {
    if handle.is_null() || message.response.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: both pointers must reference live objects for this call.
    let handle = unsafe { &*handle };
    let message = unsafe { (&*message.response).clone() };

    match handle.inner.try_send_to_channel(conn_id, message) {
        Ok(()) => MMA_OK,
        Err(_) => MMA_SERVER_ERROR,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_handle_change_config(
    handle: *const MMA_ServerHandle,
    config: *const MMA_ServerConfig,
) -> i32 {
    if handle.is_null() || config.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: pointers must reference live objects for this call.
    let handle = unsafe { &*handle };
    let config = unsafe { &*config };

    if config.bind_ip.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    let Some(config) = config_from_c(config) else {
        return MMA_INVALID_CONFIG;
    };

    if config.frame_config.validate().is_err() || config.validate().is_err() {
        return MMA_INVALID_CONFIG;
    }

    match handle.inner.change_config(config) {
        Ok(()) => MMA_OK,
        Err(error) => {
            eprintln!("MMA config update failed: {error}");
            MMA_SERVER_ERROR
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_local_port(server: *const MMA_Server, port: *mut u16) -> i32 {
    if server.is_null() || port.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: server is live and port points to writable u16 storage.
    unsafe { *port = (*server).bound_port };
    MMA_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_stop(server: *mut MMA_Server) -> i32 {
    if server.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: server must be a live handle exclusively accessible during stop.
    let server = unsafe { &mut *server };
    let Some(thread) = server.thread.take() else {
        return MMA_OK;
    };

    server.stop.cancel();
    if thread.join().is_err() {
        return MMA_SERVER_ERROR;
    }

    MMA_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_destroy(server: *mut MMA_Server) {
    if !server.is_null() {
        // SAFETY: forwards the live pointer to stop, then reclaims its allocation.
        let _ = unsafe { mma_server_stop(server) };
        unsafe { drop(Box::from_raw(server)) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mma_response_new(request_id: u32) -> MMA_Response {
    MMA_Response {
        response: Box::into_raw(Box::new(Response::new(request_id))),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_response_destroy(response: MMA_Response) {
    if !response.response.is_null() {
        // SAFETY: response must be a live, uniquely-owned response handle.
        unsafe { drop(Box::from_raw(response.response)) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_response_set_status(
    response: MMA_Response,
    opcode: u8,
) -> MMA_Response {
    let Some(opcode) = ResponseOpcode::from_number(opcode) else {
        return response_error();
    };

    let response_ptr = response.response;
    let response = match unsafe { response_mut(response) } {
        Ok(response) => response,
        Err(_) => return response_error(),
    };

    response.headers.opcode = opcode;
    MMA_Response {
        response: response_ptr,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_response_set_payload(
    response: MMA_Response,
    data: *const u8,
    data_len: usize,
) -> MMA_Response {
    if !valid_slice(data, data_len) {
        return response_error();
    }

    let response_ptr = response.response;
    let response = match unsafe { response_mut(response) } {
        Ok(response) => response,
        Err(_) => return response_error(),
    };

    // SAFETY: data/data_len were validated; caller provides readable bytes.
    response.payload = Bytes::copy_from_slice(unsafe { raw_slice(data, data_len) });
    MMA_Response {
        response: response_ptr,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_response_add_option(
    response: MMA_Response,
    key: *const u8,
    key_len: usize,
    value: *const u8,
    value_len: usize,
) -> MMA_Response {
    if !valid_slice(key, key_len)
        || !valid_slice(value, value_len)
        || key_len == 0
        || value_len == 0
        || key_len > u16::MAX as usize
        || value_len > u16::MAX as usize
    {
        return response_error();
    }

    // SAFETY: key/value slices were validated and are copied into owned Bytes.
    let key = unsafe { raw_slice(key, key_len) };
    let value = unsafe { raw_slice(value, value_len) };

    let response_ptr = response.response;
    let response = match unsafe { response_mut(response) } {
        Ok(response) => response,
        Err(_) => return response_error(),
    };

    if response.options.len() >= u8::MAX as usize {
        return response_error();
    }

    response
        .options
        .push((Bytes::copy_from_slice(key), Bytes::copy_from_slice(value)));

    MMA_Response {
        response: response_ptr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_usable() {
        let mut config = std::mem::MaybeUninit::uninit();
        assert_eq!(
            unsafe { mma_server_config_default(config.as_mut_ptr()) },
            MMA_OK
        );

        let config = unsafe { config.assume_init() };
        assert_eq!(config.port, 8080);
        assert_eq!(config.max_connections, 128_000);
        assert_eq!(config.drain_timeout_ms, 2_000);
        assert_eq!(config.shutdown_grace_ms, 10_000);
        assert_eq!(config.request_timeout_ms, 10_000);
        assert!(
            framer_config_from_c(config.framer)
                .unwrap()
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn duration_options_are_converted_to_rust_config() {
        let mut config = std::mem::MaybeUninit::uninit();
        assert_eq!(
            unsafe { mma_server_config_default(config.as_mut_ptr()) },
            MMA_OK
        );

        let mut config = unsafe { config.assume_init() };
        config.drain_timeout_ms = 125;
        config.shutdown_grace_ms = 4_500;
        config.request_timeout_ms = 0;

        let rust_config = config_from_c(&config).expect("default bind address is valid");
        assert_eq!(rust_config.drain_timeout, Duration::from_millis(125));
        assert_eq!(rust_config.shutdown_grace, Duration::from_millis(4_500));
        assert_eq!(rust_config.request_timeout, Duration::ZERO);
    }

    #[test]
    fn null_arguments_are_rejected() {
        assert_eq!(
            unsafe { mma_server_config_default(ptr::null_mut()) },
            MMA_INVALID_ARGUMENT
        );

        let mut server = ptr::null_mut();
        assert_eq!(
            unsafe { mma_server_create(ptr::null(), &mut server) },
            MMA_INVALID_ARGUMENT
        );

        assert_eq!(
            unsafe { mma_server_stop(ptr::null_mut()) },
            MMA_INVALID_ARGUMENT
        );
    }
}
