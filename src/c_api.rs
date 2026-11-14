
#![allow(non_camel_case_types)]

use crate::config::config::{
    BigBytesConfig,
    Config,
    FramerConfig,
    TlsConfig,
};

use crate::core::commands::{
    Request,
    Response,
    ResponseOpcode,
};

use crate::core::handle::ServerHandle;
use crate::core::router::Router;
use crate::core::server::Server;

use bytes::Bytes;

use std::ffi::{
    CStr,
    c_char,
    c_void,
};

use std::net::{
    IpAddr,
    SocketAddr,
};

use std::ptr;
use std::str::FromStr;

use std::sync::{
    Arc,
    Mutex,
};

use std::sync::mpsc;

use std::thread::{
    self,
    JoinHandle,
};

use std::time::Duration;

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;


// ============================================================
// Status codes
// ============================================================

pub const MMA_OK: i32 = 0;
pub const MMA_INVALID_ARGUMENT: i32 = -1;
pub const MMA_INVALID_CONFIG: i32 = -2;
pub const MMA_SERVER_ERROR: i32 = -3;
pub const MMA_SERVER_ALREADY_STARTED: i32 = -4;
pub const MMA_REQUEST_ALREADY_COMPLETED: i32 = -5;
pub const MMA_REQUEST_CANCELLED: i32 = -6;


// ============================================================
// ABI structures
// ============================================================

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MMA_BigBytesConfig {
    pub options_len_order: u8,
    pub payload_len_order: u8,
    pub req_id_order: u8,
}

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

    pub drain_timeout_ms: u64,
    pub shutdown_grace_ms: u64,
    pub request_timeout_ms: u64,

    pub tls_config: *const MMA_TLSConfig,
}

#[repr(C)]
pub struct MMA_Request {
    _private: [u8; 0],
}

/*
 * MMA_Response is opaque to C.
 * Its memory is allocated and released by this library.
 */
pub struct MMA_Response {
    inner: Response,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MMA_RequestInfo {
    pub route: *const u8,
    pub route_len: usize,

    pub opcode: u8,
    pub version: u8,

    pub request_id: u32,

    pub payload: *const u8,
    pub payload_len: usize,
}

pub struct MMA_ServerHandle {
    inner: ServerHandle,
}


// ============================================================
// Callback types
// ============================================================

pub type MMA_RouteCallback =
    Option<
        extern "C" fn(
            *mut MMA_Request,
            *mut c_void,
        )
    >;

pub type MMA_ChannelCallback =
    Option<
        extern "C" fn(
            *mut MMA_Request,
            u32,
            *mut c_void,
        )
    >;


// ============================================================
// Internal asynchronous request
// ============================================================

struct RequestState {
    /*
     * Request owns all backing buffers.
     *
     * C receives borrowed pointers into this object.
     * Those pointers remain valid while the corresponding
     * MMA_AsyncRequest Arc reference is alive.
     */
    request: Request,

    /*
     * Only route requests have a response sender.
     * Channel/lifecycle events are fire-and-forget notifications.
     */
    response_tx: Mutex<Option<oneshot::Sender<Box<MMA_Response>>>>,

    cancellation: CancellationToken,

    expects_response: bool,
}


/*
 * Drops cancellation token when the route future is cancelled.
 *
 * This is important because C may still hold an Arc reference
 * after the Tokio route future itself has been dropped.
 */
struct CancellationGuard {
    token: CancellationToken,
    armed: bool,
}

impl CancellationGuard {
    fn new(token: CancellationToken) -> Self {
        Self {
            token,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        if self.armed {
            self.token.cancel();
        }
    }
}


/*
 * The C pointer is the data pointer of Arc<AsyncRequestState>.
 *
 * C must only use pointers obtained from an active callback,
 * or pointers for which it owns a retained Arc reference.
 */
unsafe fn request_state<'a>(
    request: *const MMA_Request,
) -> Option<&'a RequestState> {
    if request.is_null() {
        return None;
    }

    let state = request.cast::<RequestState>();

    // SAFETY:
    // The caller must pass a live MMA_AsyncRequest handle.
    unsafe { state.as_ref() }
}


// ============================================================
// Registered handlers
// ============================================================

struct RegisteredRoute {
    route: String,
    callback: extern "C" fn(
        *mut MMA_Request,
        *mut c_void,
    ),
    user_data: usize,
}

struct RegisteredChannelHandler {
    route: String,
    callback: extern "C" fn(
        *mut MMA_Request,
        u32,
        *mut c_void,
    ),
    user_data: usize,
}

struct RegisteredLifecycleHandler {
    callback: extern "C" fn(
        *mut MMA_Request,
        u32,
        *mut c_void,
    ),
    user_data: usize,
}


// ============================================================
// Server
// ============================================================

pub struct MMA_Server {
    config: Config,

    bound_port: u16,

    routes: Vec<RegisteredRoute>,

    channel_handlers: Vec<RegisteredChannelHandler>,

    on_channel_connect_handler:
        Option<RegisteredLifecycleHandler>,

    on_channel_disconnect_handler:
        Option<RegisteredLifecycleHandler>,

    started: bool,

    stop: CancellationToken,

    thread: Option<JoinHandle<()>>,

    handle: Option<ServerHandle>,
}


// ============================================================
// Utility
// ============================================================

fn valid_slice<T>(
    data: *const T,
    len: usize,
) -> bool {
    len == 0
        || (
            !data.is_null()
            && len <= isize::MAX as usize
        )
}

unsafe fn raw_slice<'a, T>(
    data: *const T,
    len: usize,
) -> &'a [T] {
    if len == 0 {
        &[]
    } else {
        // SAFETY:
        // C must provide a readable, aligned buffer.
        unsafe {
            std::slice::from_raw_parts(data, len)
        }
    }
}


// ============================================================
// Framer configuration conversion
// ============================================================

fn framer_config_from_c(
    config: MMA_FramerConfig,
) -> Option<FramerConfig> {
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
            options_len_order:
                config.big_bytes_config.options_len_order,

            payload_len_order:
                config.big_bytes_config.payload_len_order,

            req_id_order:
                config.big_bytes_config.req_id_order,
        },
    })
}


fn framer_config_to_c(
    config: &FramerConfig,
) -> MMA_FramerConfig {
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
            options_len_order:
                config.big_bytes_config.options_len_order,

            payload_len_order:
                config.big_bytes_config.payload_len_order,

            req_id_order:
                config.big_bytes_config.req_id_order,
        },
    }
}


// ============================================================
// Server configuration conversion
// ============================================================

fn config_from_c(
    config: &MMA_ServerConfig,
) -> Option<Config> {
    if config.bind_ip.is_null() {
        return None;
    }

    // SAFETY:
    // bind_ip must be a valid NUL-terminated C string.
    let ip_str = unsafe {
        CStr::from_ptr(config.bind_ip)
    }
    .to_str()
    .ok()?;

    let ip_addr = IpAddr::from_str(ip_str).ok()?;

    let tls_config = if config.tls_config.is_null() {
        None
    } else {
        // SAFETY:
        // C must provide a valid MMA_TLSConfig.
        let tls = unsafe {
            &*config.tls_config
        };

        if tls.tls_certificate_path.is_null()
            || tls.tls_key_path.is_null()
        {
            return None;
        }

        let cert = unsafe {
            CStr::from_ptr(tls.tls_certificate_path)
        }
        .to_str()
        .ok()?
        .to_owned();

        let key = unsafe {
            CStr::from_ptr(tls.tls_key_path)
        }
        .to_str()
        .ok()?
        .to_owned();

        Some(TlsConfig {
            tls_certificate_path: cert,
            tls_key_path: key,
            tls_handshake_timeout:
                Duration::from_millis(
                    tls.tls_handshake_timeout
                ),
        })
    };

    Some(Config {
        address: SocketAddr::new(
            ip_addr,
            config.port,
        ),

        frame_config:
            framer_config_from_c(config.framer)?,

        max_batch: config.max_batch,

        max_connections: config.max_connections,

        max_in_flight: config.max_in_flight,

        drain_timeout:
            Duration::from_millis(config.drain_timeout_ms),

        shutdown_grace:
            Duration::from_millis(config.shutdown_grace_ms),

        request_timeout:
            Duration::from_millis(config.request_timeout_ms),

        tls_config,
    })
}


// ============================================================
// Async request: route dispatch
// ============================================================

async fn invoke_route(
    callback: extern "C" fn(
        *mut MMA_Request,
        *mut c_void,
    ),
    request: Request,
    user_data: usize,
) -> Response {
    let request_id = request.headers.req_id;

    let (response_tx, response_rx) =
        oneshot::channel::<Box<MMA_Response>>();

    let cancellation = CancellationToken::new();

    let state = Arc::new(RequestState {
        request,

        response_tx: Mutex::new(
            Some(response_tx)
        ),

        cancellation: cancellation.clone(),

        expects_response: true,
    });

    let mut cancellation_guard =
        CancellationGuard::new(cancellation);

    /*
     * Borrowed reference held by this future.
     *
     * C may call retain() before this callback returns.
     */
    let request_handle =
        Arc::as_ptr(&state) as *mut MMA_Request;

    /*
     * Important:
     * The callback must return quickly.
     * It must schedule I/O/CPU work elsewhere.
     */
    callback(
        request_handle,
        user_data as *mut c_void,
    );

    /*
     * Tokio waits asynchronously here.
     * No OS thread is occupied while C performs I/O.
     */
    match response_rx.await {
        Ok(response) => {
            cancellation_guard.disarm();

            response.inner
        }

        Err(_) => {
            Response::internal_error(request_id)
        }
    }
}


// ============================================================
// Async request: channel/lifecycle dispatch
// ============================================================

fn invoke_async_channel(
    callback: extern "C" fn(
        *mut MMA_Request,
        u32,
        *mut c_void,
    ),
    request: Request,
    conn_id: u32,
    user_data: usize,
) {
    let state = Arc::new(RequestState {
        request,

        response_tx: Mutex::new(None),

        cancellation: CancellationToken::new(),

        expects_response: false,
    });

    let request_handle =
        Arc::as_ptr(&state) as *mut MMA_Request;

    /*
     * The callback should enqueue work and return.
     * If it needs request data later, it must retain the handle.
     */
    callback(
        request_handle,
        conn_id,
        user_data as *mut c_void,
    );
}


// ============================================================
// Router construction
// ============================================================

fn router_from_registrations(
    routes: &[RegisteredRoute],

    channel_handlers: &[RegisteredChannelHandler],

    on_channel_connect_handler:
        Option<&RegisteredLifecycleHandler>,

    on_channel_disconnect_handler:
        Option<&RegisteredLifecycleHandler>,
) -> Result<Router, String> {
    let mut router = Router::new();

    // --------------------------------------------------------
    // Routes
    // --------------------------------------------------------

    for route in routes {
        let callback = route.callback;
        let user_data = route.user_data;

        router
            .register_route(
                route.route.clone(),

                move |request: Request, _response: Response| {
                    async move {
                        invoke_route(
                            callback,
                            request,
                            user_data,
                        )
                        .await
                    }
                },
            )
            .map_err(|error| {
                format!(
                    "route {:?}: {error}",
                    route.route
                )
            })?;
    }

    // --------------------------------------------------------
    // Channel handlers
    // --------------------------------------------------------

    for handler in channel_handlers {
        let callback = handler.callback;
        let user_data = handler.user_data;

        router
            .register_channel_handler(
                handler.route.clone(),

                move |request: Request, conn_id: u32| {
                    async move {
                        invoke_async_channel(
                            callback,
                            request,
                            conn_id,
                            user_data,
                        );
                    }
                },
            )
            .map_err(|error| {
                format!(
                    "channel route {:?}: {error}",
                    handler.route
                )
            })?;
    }

    // --------------------------------------------------------
    // On channel connect
    // --------------------------------------------------------

    if let Some(handler) = on_channel_connect_handler {
        let callback = handler.callback;
        let user_data = handler.user_data;

        router.register_on_channel_connect_handler(
            move |request: Request, conn_id: u32| {
                async move {
                    invoke_async_channel(
                        callback,
                        request,
                        conn_id,
                        user_data,
                    );
                }
            },
        );
    }

    // --------------------------------------------------------
    // On channel disconnect
    // --------------------------------------------------------

    if let Some(handler) = on_channel_disconnect_handler {
        let callback = handler.callback;
        let user_data = handler.user_data;

        router.register_on_channel_disconnect_handler(
            move |request: Request, conn_id: u32| {
                async move {
                    invoke_async_channel(
                        callback,
                        request,
                        conn_id,
                        user_data,
                    );
                }
            },
        );
    }

    Ok(router)
}


// ============================================================
// Server configuration defaults
// ============================================================

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_config_default(
    output: *mut MMA_ServerConfig,
) -> i32 {
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

    // SAFETY: output must point to writable storage.
    unsafe {
        *output = config;
    }

    MMA_OK
}


// ============================================================
// Server creation
// ============================================================

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_create(
    config: *const MMA_ServerConfig,
    output: *mut *mut MMA_Server,
) -> i32 {
    if output.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    unsafe {
        *output = ptr::null_mut();
    }

    if config.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    // SAFETY: caller provides an initialized config.
    let config = unsafe {
        *config
    };

    let Some(rust_config) = config_from_c(&config)
    else {
        return MMA_INVALID_CONFIG;
    };

    if rust_config.frame_config.validate().is_err()
        || rust_config.validate().is_err()
    {
        return MMA_INVALID_CONFIG;
    }

    let address = rust_config.address;

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

    unsafe {
        *output = Box::into_raw(
            Box::new(server)
        );
    }

    MMA_OK
}


// ============================================================
// Route registration
// ============================================================

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_register_route(
    server: *mut MMA_Server,

    route: *const u8,

    route_len: usize,

    callback: MMA_RouteCallback,

    user_data: *mut c_void,
) -> i32 {
    if server.is_null()
        || !valid_slice(route, route_len)
    {
        return MMA_INVALID_ARGUMENT;
    }

    let server = unsafe {
        &mut *server
    };

    if server.started {
        return MMA_SERVER_ALREADY_STARTED;
    }

    if route_len == 0
        || route_len > u8::MAX as usize
    {
        return MMA_INVALID_ARGUMENT;
    }

    let route = match std::str::from_utf8(
        unsafe {
            raw_slice(route, route_len)
        }
    ) {
        Ok(value) => value,

        Err(_) => {
            return MMA_INVALID_ARGUMENT;
        }
    };

    let Some(callback) = callback else {
        return MMA_INVALID_ARGUMENT;
    };

    server.routes.push(
        RegisteredRoute {
            route: route.to_owned(),

            callback,

            user_data: user_data as usize,
        }
    );

    MMA_OK
}


// ============================================================
// Channel handler registration
// ============================================================

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_register_channel_handler(
    server: *mut MMA_Server,

    route: *const u8,

    route_len: usize,

    callback: MMA_ChannelCallback,

    user_data: *mut c_void,
) -> i32 {
    if server.is_null()
        || !valid_slice(route, route_len)
    {
        return MMA_INVALID_ARGUMENT;
    }

    let server = unsafe {
        &mut *server
    };

    if server.started {
        return MMA_SERVER_ALREADY_STARTED;
    }

    if route_len == 0
        || route_len > u8::MAX as usize
    {
        return MMA_INVALID_ARGUMENT;
    }

    let route = match std::str::from_utf8(
        unsafe {
            raw_slice(route, route_len)
        }
    ) {
        Ok(value) => value,

        Err(_) => {
            return MMA_INVALID_ARGUMENT;
        }
    };

    let Some(callback) = callback else {
        return MMA_INVALID_ARGUMENT;
    };

    server.channel_handlers.push(
        RegisteredChannelHandler {
            route: route.to_owned(),

            callback,

            user_data: user_data as usize,
        }
    );

    MMA_OK
}


// ============================================================
// Lifecycle registration
// ============================================================

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_register_on_channel_connect_handler(
    server: *mut MMA_Server,

    callback: MMA_ChannelCallback,

    user_data: *mut c_void,
) -> i32 {
    if server.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    let server = unsafe {
        &mut *server
    };

    if server.started {
        return MMA_SERVER_ALREADY_STARTED;
    }

    let Some(callback) = callback else {
        return MMA_INVALID_ARGUMENT;
    };

    server.on_channel_connect_handler =
        Some(RegisteredLifecycleHandler {
            callback,
            user_data: user_data as usize,
        });

    MMA_OK
}


#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_register_on_channel_disconnect_handler(
    server: *mut MMA_Server,

    callback: MMA_ChannelCallback,

    user_data: *mut c_void,
) -> i32 {
    if server.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    let server = unsafe {
        &mut *server
    };

    if server.started {
        return MMA_SERVER_ALREADY_STARTED;
    }

    let Some(callback) = callback else {
        return MMA_INVALID_ARGUMENT;
    };

    server.on_channel_disconnect_handler =
        Some(RegisteredLifecycleHandler {
            callback,
            user_data: user_data as usize,
        });

    MMA_OK
}


// ============================================================
// Server start
// ============================================================

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_start(
    server: *mut MMA_Server,
) -> i32 {
    if server.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    let server = unsafe {
        &mut *server
    };

    if server.started {
        return MMA_SERVER_ALREADY_STARTED;
    }

    let config = server.config.clone();

    let router = match router_from_registrations(
        &server.routes,
        &server.channel_handlers,
        server.on_channel_connect_handler.as_ref(),
        server.on_channel_disconnect_handler.as_ref(),
    ) {
        Ok(router) => router,

        Err(error) => {
            eprintln!(
                "MMA router initialization failed: {error}"
            );

            return MMA_SERVER_ERROR;
        }
    };

    let stop = server.stop.clone();

    let (ready_tx, ready_rx) =
        mpsc::sync_channel::<
            Result<(SocketAddr, ServerHandle), String>
        >(1);

    /*
     * One dedicated OS thread owns the Tokio runtime.
     *
     * No blocking C callbacks are executed on this thread.
     */
    let thread = match thread::Builder::new()
        .name("mma-runtime".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,

                Err(error) => {
                    let _ = ready_tx.send(
                        Err(format!(
                            "Tokio runtime initialization failed: {error}"
                        ))
                    );

                    return;
                }
            };

            runtime.block_on(async move {
                let listener =
                    match tokio::net::TcpListener::bind(
                        config.address
                    )
                    .await
                    {
                        Ok(listener) => listener,

                        Err(error) => {
                            let _ = ready_tx.send(
                                Err(format!(
                                    "TCP bind failed: {error}"
                                ))
                            );

                            return;
                        }
                    };

                let local_addr = match listener.local_addr() {
                    Ok(addr) => addr,

                    Err(error) => {
                        let _ = ready_tx.send(
                            Err(format!(
                                "Cannot obtain listener address: {error}"
                            ))
                        );

                        return;
                    }
                };

                let mut running_server =
                    match Server::from_listener(
                        config,
                        listener,
                    ) {
                        Ok(server) => server,

                        Err(error) => {
                            let _ = ready_tx.send(
                                Err(format!(
                                    "Server initialization failed: {error}"
                                ))
                            );

                            return;
                        }
                    };

                let handle = running_server.handle();

                if ready_tx.send(
                    Ok((local_addr, handle))
                ).is_err() {
                    return;
                }

                if let Err(error) = running_server
                    .run_until(router, stop.cancelled())
                    .await
                {
                    eprintln!(
                        "MMA server stopped after an error: {error}"
                    );
                }
            });
        })
    {
        Ok(thread) => thread,

        Err(error) => {
            eprintln!(
                "Failed to create MMA runtime thread: {error}"
            );

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
            eprintln!(
                "MMA server startup failed: {error}"
            );

            let _ = thread.join();

            MMA_SERVER_ERROR
        }

        Err(error) => {
            eprintln!(
                "MMA startup handshake failed: {error}"
            );

            let _ = thread.join();

            MMA_SERVER_ERROR
        }
    }
}


// ============================================================
// Async request refcount
// ============================================================

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_request_retain(
    request: *mut MMA_Request,
) -> i32 {
    if request.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    /*
     * The callback currently holds at least one Arc reference,
     * so increment_strong_count is valid during the callback.
     */
    unsafe {
        Arc::increment_strong_count(
            request.cast::<RequestState>()
        );
    }

    MMA_OK
}


#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_request_release(
    request: *mut MMA_Request,
) {
    if !request.is_null() {
        /*
         * Must be called exactly once for every successful retain().
         */
        unsafe {
            Arc::decrement_strong_count(
                request.cast::<RequestState>()
            );
        }
    }
}


// ============================================================
// Async request information
// ============================================================

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_request_get_info(
    request: *const MMA_Request,

    output: *mut MMA_RequestInfo,
) -> i32 {
    if request.is_null() || output.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    let Some(state) = (
        unsafe {
            request_state(request)
        }
    ) else {
        return MMA_INVALID_ARGUMENT;
    };

    let req = &state.request;

    let info = MMA_RequestInfo {
        route: req.route.as_ptr(),
        route_len: req.route.len(),

        opcode: req.headers.opcode as u8,
        version: req.headers.version,

        request_id: req.headers.req_id,

        payload: req.payload.as_ptr(),
        payload_len: req.payload.len(),
    };

    unsafe {
        *output = info;
    }

    MMA_OK
}


#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_request_option_count(
    request: *const MMA_Request,

    output: *mut usize,
) -> i32 {
    if request.is_null() || output.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    let Some(state) = (
        unsafe {
            request_state(request)
        }
    ) else {
        return MMA_INVALID_ARGUMENT;
    };

    unsafe {
        *output = state.request.options.len();
    }

    MMA_OK
}


#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_request_option_at(
    request: *const MMA_Request,

    index: usize,

    key: *mut *const u8,
    key_len: *mut usize,

    value: *mut *const u8,
    value_len: *mut usize,
) -> i32 {
    if request.is_null()
        || key.is_null()
        || key_len.is_null()
        || value.is_null()
        || value_len.is_null()
    {
        return MMA_INVALID_ARGUMENT;
    }

    let Some(state) = (
        unsafe {
            request_state(request)
        }
    ) else {
        return MMA_INVALID_ARGUMENT;
    };

    let Some((option_key, option_value)) =
        state.request.options.get(index)
    else {
        return MMA_INVALID_ARGUMENT;
    };

    unsafe {
        *key = option_key.as_ptr();
        *key_len = option_key.len();

        *value = option_value.as_ptr();
        *value_len = option_value.len();
    }

    MMA_OK
}


// ============================================================
// Request cancellation
// ============================================================

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_request_is_cancelled(
    request: *const MMA_Request,

    output: *mut u8,
) -> i32 {
    if request.is_null() || output.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    let Some(state) = (
        unsafe {
            request_state(request)
        }
    ) else {
        return MMA_INVALID_ARGUMENT;
    };

    unsafe {
        *output = u8::from(
            state.cancellation.is_cancelled()
        );
    }

    MMA_OK
}


// ============================================================
// Request completion
// ============================================================

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_request_complete(
    request: *mut MMA_Request,

    response: *mut MMA_Response,
) -> i32 {
    if request.is_null() || response.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    let Some(state) = (
        unsafe {
            request_state(request)
        }
    ) else {
        return MMA_INVALID_ARGUMENT;
    };

    if !state.expects_response {
        return MMA_INVALID_ARGUMENT;
    }

    /*
     * Taking the sender is the single-completion operation.
     * Mutex guarantees that two concurrent completion attempts
     * cannot both obtain the sender.
     */
    let sender = {
        let mut guard = state
            .response_tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        guard.take()
    };

    let Some(sender) = sender else {
        /*
         * The request was already completed.
         * For a valid route request, response ownership is consumed.
         */
        unsafe {
            drop(Box::from_raw(response));
        }

        return MMA_REQUEST_ALREADY_COMPLETED;
    };

    /*
     * Consume the C response handle.
     */
    let response = unsafe {
        Box::from_raw(response)
    };

    match sender.send(response) {
        Ok(()) => MMA_OK,

        /*
         * The receiver was dropped, usually because the request
         * timed out or was cancelled during shutdown.
         *
         * The returned Box is intentionally dropped here.
         */
        Err(_response) => MMA_REQUEST_CANCELLED,
    }
}


// ============================================================
// Response construction
// ============================================================

#[unsafe(no_mangle)]
pub extern "C" fn mma_response_new(
    request_id: u32,
) -> *mut MMA_Response {
    let response = MMA_Response {
        inner: Response::new(request_id),
    };

    Box::into_raw(
        Box::new(response)
    )
}


#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_response_set_status(
    response: *mut MMA_Response,

    opcode: u8,
) -> i32 {
    if response.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    let Some(opcode) = ResponseOpcode::from_number(opcode)
    else {
        return MMA_INVALID_ARGUMENT;
    };

    let response = unsafe {
        &mut *response
    };

    response.inner.headers.opcode = opcode;

    MMA_OK
}


#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_response_set_payload(
    response: *mut MMA_Response,

    data: *const u8,
    data_len: usize,
) -> i32 {
    if response.is_null()
        || !valid_slice(data, data_len)
    {
        return MMA_INVALID_ARGUMENT;
    }

    let payload = unsafe {
        raw_slice(data, data_len)
    };

    let response = unsafe {
        &mut *response
    };

    /*
     * This API copies C memory into Bytes.
     *
     * A separate ownership-transfer buffer API can be added
     * later without changing the async completion mechanism.
     */
    response.inner.payload =
        Bytes::copy_from_slice(payload);

    MMA_OK
}


#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_response_add_option(
    response: *mut MMA_Response,

    key: *const u8,
    key_len: usize,

    value: *const u8,
    value_len: usize,
) -> i32 {
    if response.is_null()
        || !valid_slice(key, key_len)
        || !valid_slice(value, value_len)
        || key_len == 0
        || value_len == 0
        || key_len > u16::MAX as usize
        || value_len > u16::MAX as usize
    {
        return MMA_INVALID_ARGUMENT;
    }

    let response = unsafe {
        &mut *response
    };

    if response.inner.options.len() >= u8::MAX as usize {
        return MMA_INVALID_ARGUMENT;
    }

    let key = unsafe {
        raw_slice(key, key_len)
    };

    let value = unsafe {
        raw_slice(value, value_len)
    };

    response.inner.options.push((
        Bytes::copy_from_slice(key),
        Bytes::copy_from_slice(value),
    ));

    MMA_OK
}


#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_response_destroy(
    response: *mut MMA_Response,
) {
    if !response.is_null() {
        unsafe {
            drop(Box::from_raw(response));
        }
    }
}


// ============================================================
// Server handle
// ============================================================

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_get_handle(
    server: *const MMA_Server,

    output: *mut *mut MMA_ServerHandle,
) -> i32 {
    if server.is_null() || output.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    unsafe {
        *output = ptr::null_mut();
    }

    let server = unsafe {
        &*server
    };

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

    let handle = Box::new(
        MMA_ServerHandle {
            inner: handle.clone(),
        }
    );

    unsafe {
        *output = Box::into_raw(handle);
    }

    MMA_OK
}


#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_handle_destroy(
    handle: *mut MMA_ServerHandle,
) {
    if !handle.is_null() {
        unsafe {
            drop(Box::from_raw(handle));
        }
    }
}


#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_handle_send_to_channel(
    handle: *const MMA_ServerHandle,

    conn_id: u32,

    message: *const MMA_Response,
) -> i32 {
    if handle.is_null() || message.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    let handle = unsafe {
        &*handle
    };

    let message = unsafe {
        &*message
    };

    /*
     * Bytes are reference-counted.
     * The Response clone only clones its owning containers.
     */
    let message = message.inner.clone();

    match handle.inner.try_send_to_channel(
        conn_id,
        message,
    ) {
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

    let handle = unsafe {
        &*handle
    };

    let config = unsafe {
        &*config
    };

    let Some(config) = config_from_c(config) else {
        return MMA_INVALID_CONFIG;
    };

    if config.frame_config.validate().is_err()
        || config.validate().is_err()
    {
        return MMA_INVALID_CONFIG;
    }

    match handle.inner.change_config(config) {
        Ok(()) => MMA_OK,

        Err(error) => {
            eprintln!(
                "MMA config update failed: {error}"
            );

            MMA_SERVER_ERROR
        }
    }
}


// ============================================================
// Local server port
// ============================================================

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_local_port(
    server: *const MMA_Server,

    port: *mut u16,
) -> i32 {
    if server.is_null() || port.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    unsafe {
        *port = (*server).bound_port;
    }

    MMA_OK
}


// ============================================================
// Stop / destroy
// ============================================================

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_stop(
    server: *mut MMA_Server,
) -> i32 {
    if server.is_null() {
        return MMA_INVALID_ARGUMENT;
    }

    let server = unsafe {
        &mut *server
    };

    server.stop.cancel();

    let Some(thread) = server.thread.take() else {
        return MMA_OK;
    };

    if thread.join().is_err() {
        return MMA_SERVER_ERROR;
    }

    server.handle = None;

    MMA_OK
}


#[unsafe(no_mangle)]
pub unsafe extern "C" fn mma_server_destroy(
    server: *mut MMA_Server,
) {
    if !server.is_null() {
        let _ = unsafe {
            mma_server_stop(server)
        };

        unsafe {
            drop(Box::from_raw(server));
        }
    }
}
