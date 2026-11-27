use crate::config::config::Config;
use crate::core::commands::ProtocolError;
use crate::core::commands::Request;
use crate::core::commands::RequestOpcode;
use crate::core::commands::Response;
use crate::core::commands::SocketStatus;
use crate::core::router::ReservedRoutes;
use crate::core::router::Router;
use crate::protocol::framer::Framer;
use bytes::Bytes;
use futures_util::FutureExt;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use std::panic;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::watch;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub trait AsyncConnection: AsyncRead + AsyncWrite + Unpin + Send + 'static {}

impl<T> AsyncConnection for T where T: AsyncRead + AsyncWrite + Unpin + Send + 'static {}

struct ConnectionContext<F, T> {
    pub on_channel_connect: F,
    pub on_channel_disconnect: T,
    pub socket_status: AtomicU64,
    pub disconnect: CancellationToken,
    pub channel_task: Mutex<Option<JoinHandle<()>>>,
    pub response_tx: Sender<Response>,
    pub router: Arc<Router>,
}

impl<F, T> ConnectionContext<F, T>
where
    F: Fn() -> Result<(SocketStatus, mpsc::Receiver<Response>), String> + Send + Sync,
    T: Fn(u32) -> Result<(), String> + Send + Sync,
{
    pub async fn register_connect(&self, req: Request) -> Result<(), String> {
        if !matches!(
            SocketStatus::from_u64(self.socket_status.load(Ordering::Relaxed)),
            SocketStatus::Fresh
        ) {
            return Err(String::from("Socket isn't fresh"));
        }

        if change_socket_status(
            SocketStatus::Fresh,
            SocketStatus::Connecting,
            &self.socket_status,
        )
        .is_err()
        {
            return Err(String::from("Socket isn't fresh"));
        }

        let (status, mut receiver) = match (self.on_channel_connect)() {
            Ok(s) => s,
            Err(e) => {
                let _ = change_socket_status(
                    SocketStatus::Connecting,
                    SocketStatus::Fresh,
                    &self.socket_status,
                );
                return Err(format!("Error with connecting to channel: {e}"));
            }
        };

        let conn_id = if let SocketStatus::Channel(id) = status {
            id
        } else {
            let _ = change_socket_status(
                SocketStatus::Connecting,
                SocketStatus::Fresh,
                &self.socket_status,
            );
            return Err(String::from("Cannot connect to channel"));
        };

        if AssertUnwindSafe(self.router.on_channel_connect(req, conn_id))
            .catch_unwind()
            .await
            .is_err()
        {
            let _ = change_socket_status(
                SocketStatus::Connecting,
                SocketStatus::Fresh,
                &self.socket_status,
            );
            if let Err(_e) = (self.on_channel_disconnect)(conn_id) {
                eprintln!("Error with cancelling of channel connection");
            }
            self.disconnect.cancel();

            return Err(String::from("Cannot complete"));
        }

        let tx_channel = self.response_tx.clone();

        let relay_task = tokio::spawn(async move {
            while let Some(message) = receiver.recv().await {
                if tx_channel.send(message).await.is_err() {
                    break;
                }
            }
        });

        match self.channel_task.lock() {
            Ok(mut slot) => {
                if let Some(previous) = slot.replace(relay_task) {
                    previous.abort();
                }
            }

            Err(_) => {
                relay_task.abort();
            }
        }

        change_socket_status(SocketStatus::Connecting, status, &self.socket_status)
            .map_err(|()| String::from("Cannot change socket status"))?;
        Ok(())
    }

    pub fn disconnect_channel(&self) {
        let status = SocketStatus::from_u64(self.socket_status.load(Ordering::Relaxed));
        if let SocketStatus::Channel(id) = status
            && let Err(e) = (self.on_channel_disconnect)(id)
        {
            eprintln!("Error with channel disconnecting: {e}");
        }
        self.disconnect.cancel();
    }
}

pub async fn serve_connection<S, F, T>(
    socket: S,
    router: Arc<Router>,
    mut config_receiver: watch::Receiver<Config>,
    semaphore: Arc<Semaphore>,
    stop: CancellationToken,
    kill: CancellationToken,
    conn_permit: OwnedSemaphorePermit,
    on_channel_connect: F,
    on_channel_disconnect: T,
) where
    S: AsyncConnection,
    F: Fn() -> Result<(SocketStatus, mpsc::Receiver<Response>), String> + Send + Sync + 'static,
    T: Fn(u32) -> Result<(), String> + Send + Sync + 'static,
{
    let mut config = config_receiver.borrow_and_update().clone();

    let framer_config = Arc::new(RwLock::new(config.frame_config.clone()));

    let framer = match Framer::new(Arc::clone(&framer_config)) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Cannot create framer: {e}");
            return;
        }
    };
    let framed = Framed::new(socket, framer);

    let (sink, mut stream) = framed.split();
    let (tx, rx) = tokio::sync::mpsc::channel::<Response>(1000);
    let inflight = TaskTracker::new();
    let mut writer = writer(sink, rx, config.max_batch, config.write_timeout);

    let context = Arc::new(ConnectionContext {
        channel_task: Mutex::new(None),
        disconnect: CancellationToken::new(),
        on_channel_connect,
        on_channel_disconnect,
        response_tx: tx.clone(),
        router,
        socket_status: AtomicU64::new(SocketStatus::to_u64(SocketStatus::Fresh)),
    });

    loop {
        let frame = tokio::select! {
            biased;
            () = kill.cancelled() => break,
            () = stop.cancelled() => break,
            () = context.disconnect.cancelled() => {
                if let SocketStatus::Channel(id) = SocketStatus::from_u64(context.socket_status.load(Ordering::Relaxed)) {
                    let _ = (context.on_channel_disconnect)(id);
                }
                break;
            },
            changed = config_receiver.changed() => {
                if changed.is_err() {
                    break;
                }

                let next_config = config_receiver
                    .borrow_and_update()
                    .clone();

                {
                    let mut current = framer_config
                        .write()
                        .expect("Framer config lock poisoned");

                    *current = next_config.frame_config.clone();
                }

                config = next_config;
                continue;
            }
            f = stream.next() => if let Some(f) = f { f } else {
                let status = SocketStatus::from_u64(context.socket_status.load(Ordering::Relaxed));
                if let SocketStatus::Channel(id) = status
                    && let Err(e) = (context.on_channel_disconnect)(id) {
                        eprintln!("Error with channel disconnecting: {e}");
                    }
                break;
            },
        };

        match frame {
            Result::Ok(cmd) => {
                let permit_global = tokio::select! {
                    () = stop.cancelled() => break,
                    global = Arc::clone(&semaphore).acquire_owned() => {
                        global.unwrap()
                    }
                };

                let tx = tx.clone();
                let req_id = cmd.headers.req_id;
                let request_timeout = config.request_timeout;
                let context = context.clone();
                inflight.spawn(async move {
                    match tokio::time::timeout(
                        request_timeout,
                        handle_cmd(cmd, permit_global, context),
                    )
                    .await
                    {
                        Ok(r) => {
                            if r.is_some()
                                && let Err(e) = tx.send(r.unwrap()).await
                            {
                                eprintln!("{e}");
                            }
                        }
                        Err(_) => {
                            if let Err(e) = tx.send(Response::timeout(req_id)).await {
                                eprintln!("{e}");
                            }
                        }
                    }
                });
            }
            Err(error) => {
                if let Err(e) = tx
                    .send(Response::bad_request(
                        match error {
                            ProtocolError::Io(_) => 0,
                            ProtocolError::MessageTooLarge { message: _, req_id } => {
                                req_id.unwrap_or_default()
                            }
                            ProtocolError::InvalidFrame { message: _, req_id } => {
                                req_id.unwrap_or_default()
                            }
                            ProtocolError::InvalidMessageType { message: _, req_id } => {
                                req_id.unwrap_or_default()
                            }
                            ProtocolError::InvalidUtf8 { req_id } => req_id.unwrap_or_default(),
                        },
                        Some(Bytes::from(error.to_string())),
                    ))
                    .await
                {
                    eprintln!("Error with sending to socket: {e:?}");
                }
                break;
            }
        }
    }

    let status = SocketStatus::from_u64(context.socket_status.load(Ordering::Relaxed));

    if let SocketStatus::Channel(id) = status
        && let Err(e) = (context.on_channel_disconnect)(id)
    {
        eprintln!("Error with channel disconnect: {e}");
    }

    inflight.close();
    tokio::select! { () = inflight.wait() => {}, () = kill.cancelled() => {} }
    if let Ok(mut slot) = context.channel_task.lock()
        && let Some(task) = slot.take()
    {
        task.abort();
    }
    drop(context);
    drop(tx);
    drop(conn_permit);
    if kill.is_cancelled() {
        writer.abort();
        let _ = writer.await;
    } else {
        if let Err(_) = tokio::time::timeout(config.drain_timeout, &mut writer).await {
            writer.abort();
            let _ = writer.await;
        }
        let _ = tokio::time::timeout(config.drain_timeout, async {
            while let Some(Result::Ok(_)) = stream.next().await {}
        })
        .await;
    }
}

async fn handle_cmd<F, T>(
    cmd: Request,
    permit: OwnedSemaphorePermit,
    context: Arc<ConnectionContext<F, T>>,
) -> Option<Response>
where
    F: Fn() -> Result<(SocketStatus, mpsc::Receiver<Response>), String> + Send + Sync + 'static,
    T: Fn(u32) -> Result<(), String> + Send + Sync + 'static,
{
    let req_id = cmd.headers.req_id;

    if let Err(e) = Router::validate_version(&cmd) {
        return Some(e);
    }

    let status = SocketStatus::from_u64(context.socket_status.load(Ordering::Relaxed));

    match status {
        SocketStatus::Fresh => match cmd.headers.opcode {
            RequestOpcode::Once => {
                if change_socket_status(
                    SocketStatus::Fresh,
                    SocketStatus::Once,
                    &context.socket_status,
                )
                .is_err()
                {
                    return None;
                }
                let response = match AssertUnwindSafe(context.router.handle(cmd))
                    .catch_unwind()
                    .await
                {
                    Ok(response) => response,
                    Err(_) => Response::internal_error(req_id),
                };

                drop(permit);

                return Some(response);
            }
            RequestOpcode::ChannelConnect => {
                if cmd.route == ReservedRoutes::CHANNEL_CONNECT.as_bytes() {
                    if let Err(e) = context.register_connect(cmd).await {
                        let mut response = Response::internal_error(req_id);
                        response.payload = Bytes::from(e);
                        return Some(response);
                    }

                    drop(permit);

                    return Some(Response::channel_connected(req_id));
                }

                drop(permit);

                return Some(Response::bad_request(
                    req_id,
                    Some(Bytes::from("Route must match the opcode")),
                ));
            }
            RequestOpcode::ChannelMessage => {
                drop(permit);
                return Some(Response::bad_request(
                    req_id,
                    Some(Bytes::from("Channel not connected")),
                ));
            }
        },
        SocketStatus::Once => {
            if cmd.headers.opcode == RequestOpcode::Once {
                let response = match AssertUnwindSafe(context.router.handle(cmd))
                    .catch_unwind()
                    .await
                {
                    Ok(response) => response,
                    Err(_) => Response::internal_error(req_id),
                };

                drop(permit);

                return Some(response);
            }
            drop(permit);
            return Some(Response::connection_type_mismatch(req_id));
        }
        SocketStatus::Channel(conn_id) => match cmd.headers.opcode {
            RequestOpcode::ChannelConnect => {
                drop(permit);
                return Some(Response::conflict(
                    req_id,
                    Bytes::from("Channel already connected"),
                ));
            }
            RequestOpcode::ChannelMessage => {
                if cmd.route == ReservedRoutes::CHANNEL_DISCONNECT.as_bytes() {
                    if AssertUnwindSafe(context.router.on_channel_disconnect(cmd.clone(), conn_id))
                        .catch_unwind()
                        .await
                        .is_err()
                    {
                        return Some(Response::internal_error(req_id));
                    }
                    context.disconnect_channel();
                    return None;
                }
                match AssertUnwindSafe(context.router.handle_channel(cmd, &conn_id))
                    .catch_unwind()
                    .await
                {
                    Result::Ok(Result::Err(r)) => {
                        drop(permit);
                        return Some(r);
                    }
                    Err(_) => {
                        drop(permit);
                        return Some(Response::internal_error(req_id));
                    }
                    _ => {}
                }
            }
            RequestOpcode::Once => {
                drop(permit);
                return Some(Response::connection_type_mismatch(req_id));
            }
        },
        SocketStatus::Connecting => {
            drop(permit);
            return Some(Response::connection_type_mismatch(req_id));
        }
    }

    None
}

fn writer<S>(
    mut sink: SplitSink<Framed<S, Framer>, Response>,
    mut rx: Receiver<Response>,
    max_batch: u32,
    timeout: Duration,
) -> tokio::task::JoinHandle<()>
where
    S: AsyncConnection,
{
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if let Err(e) = sink.feed(frame).await {
                match e {
                    ProtocolError::MessageTooLarge { message: _, req_id } => {
                        if let Err(err) = sink
                            .feed(Response::internal_error(req_id.unwrap_or_default()))
                            .await
                        {
                            eprintln!("Error sending frame: {err}");
                        }
                    }
                    ProtocolError::InvalidFrame { message: _, req_id } => {
                        if let Err(err) = sink
                            .feed(Response::internal_error(req_id.unwrap_or_default()))
                            .await
                        {
                            eprintln!("Error sending frame: {err}");
                        }
                    }
                    ProtocolError::InvalidMessageType { message: _, req_id } => {
                        if let Err(err) = sink
                            .feed(Response::internal_error(req_id.unwrap_or_default()))
                            .await
                        {
                            eprintln!("Error sending frame: {err}");
                        }
                    }
                    ProtocolError::InvalidUtf8 { req_id } => {
                        if let Err(err) = sink
                            .feed(Response::internal_error(req_id.unwrap_or_default()))
                            .await
                        {
                            eprintln!("Error sending frame: {err}");
                        }
                    }
                    _ => {
                        eprintln!("Error sending frame: {e}");
                    }
                }
                return;
            }
            let mut count = 1;
            while count < max_batch {
                match rx.try_recv() {
                    Result::Ok(x) => {
                        if let Err(e) = sink.feed(x).await {
                            match e {
                                ProtocolError::MessageTooLarge { message: _, req_id } => {
                                    if let Err(err) = sink
                                        .feed(Response::internal_error(req_id.unwrap_or_default()))
                                        .await
                                    {
                                        eprintln!("Error sending frame: {err}");
                                    }
                                }
                                ProtocolError::InvalidFrame { message: _, req_id } => {
                                    if let Err(err) = sink
                                        .feed(Response::internal_error(req_id.unwrap_or_default()))
                                        .await
                                    {
                                        eprintln!("Error sending frame: {err}");
                                    }
                                }
                                ProtocolError::InvalidMessageType { message: _, req_id } => {
                                    if let Err(err) = sink
                                        .feed(Response::internal_error(req_id.unwrap_or_default()))
                                        .await
                                    {
                                        eprintln!("Error sending frame: {err}");
                                    }
                                }
                                ProtocolError::InvalidUtf8 { req_id } => {
                                    if let Err(err) = sink
                                        .feed(Response::internal_error(req_id.unwrap_or_default()))
                                        .await
                                    {
                                        eprintln!("Error sending frame: {err}");
                                    }
                                }
                                _ => {
                                    eprintln!("Error sending frame: {e}");
                                }
                            }
                            return;
                        }
                        count += 1;
                    }
                    Err(_) => break,
                }
            }
            match tokio::time::timeout(timeout, sink.flush()).await {
                Err(_) => return,
                Ok(r) => {
                    if let Err(e) = r {
                        eprintln!("Error flushing sink: {e}");
                        return;
                    }
                }
            }
        }
        let _ = sink.close().await;
    })
}

fn change_socket_status(
    old: SocketStatus,
    new: SocketStatus,
    status: &AtomicU64,
) -> Result<(), ()> {
    if status
        .compare_exchange(
            SocketStatus::to_u64(old),
            SocketStatus::to_u64(new),
            Ordering::Relaxed,
            Ordering::Relaxed,
        )
        .is_err()
    {
        return Err(());
    }
    Ok(())
}
