use crate::core::commands::ProtocolError;
use crate::core::commands::Request;
use crate::core::commands::RequestOpcode;
use crate::core::commands::Response;
use crate::core::config::Config;
use crate::core::router::ReservedRoutes;
use crate::core::router::Router;
use crate::protocol::framer::Framer;
use bytes::Bytes;
use futures_util::FutureExt;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use std::panic;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use tokio::net::TcpStream;
use tokio::sync::mpsc::Receiver;
use tokio::sync::watch;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

#[derive(Debug, Clone, Copy)]
pub enum SocketStatus {
    Fresh,
    Once,
    Channel(u32),
}

impl SocketStatus {
    fn to_u64(self) -> u64 {
        match self {
            SocketStatus::Fresh => 0,
            SocketStatus::Once => 1_u64 << 32,
            SocketStatus::Channel(id) => (2_u64 << 32) | u64::from(id),
        }
    }

    fn from_u64(value: u64) -> Self {
        match value >> 32 {
            0 => SocketStatus::Fresh,
            1 => SocketStatus::Once,
            2 => SocketStatus::Channel(value as u32),
            _ => SocketStatus::Fresh,
        }
    }
}

pub async fn serve_connection<F, T>(
    socket: TcpStream,
    router: Arc<Router>,
    mut config_receiver: watch::Receiver<Config>,
    semaphore: Arc<Semaphore>,
    stop: CancellationToken,
    kill: CancellationToken,
    conn_permit: OwnedSemaphorePermit,
    on_channel_connect: F,
    on_channel_disconnect: T
) where
    F: Fn() -> Result<(SocketStatus, mpsc::Receiver<Response>), String> + Send + Sync + 'static,
    T: Fn(u32) -> Result<(), String> + Send + Sync + 'static
{
    let mut config = config_receiver.borrow_and_update().clone();

    let framer_config = Arc::new(RwLock::new(config.frame_config.clone()));

    let framed = Framed::new(socket, Framer::new(Arc::clone(&framer_config)));

    let (sink, mut stream) = framed.split();
    let (tx, rx) = tokio::sync::mpsc::channel::<Response>(1000);
    let inflight = TaskTracker::new();

    let conn_semaphore = Arc::new(Semaphore::new(1));
    let tx_copy = tx.clone();
    
    let on_channel_connect = Arc::new(on_channel_connect);
    let socket_status = Arc::new(AtomicU64::new(SocketStatus::to_u64(SocketStatus::Fresh)));
    let channel_task: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));
    
    let mut writer = writer(sink, rx, config.max_batch);
    let on_channel_disconnect = Arc::new(on_channel_disconnect);
    
    let disconnect = CancellationToken::new();

    loop {
        let frame = tokio::select! {
            biased;
            _ = kill.cancelled() => break,
            _ = stop.cancelled() => break,
            _ = disconnect.cancelled() => break,
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
            f = stream.next() => match f { Some(f) => f, None => {
                let status = SocketStatus::from_u64(socket_status.load(Ordering::Relaxed));
                match status {
                    SocketStatus::Channel(id) => {
                        if let Err(e) = on_channel_disconnect(id) {
                            eprintln!("Error with channel disconnecting: {}", e);
                        };
                    },
                    _ => {

                    }
                };
                break;
            }},
        };

        match frame {
            Result::Ok(cmd) => {
                let permit_global = tokio::select! {
                    _ = stop.cancelled() => break,
                    global = Arc::clone(&semaphore).acquire_owned() => {
                        global.unwrap()
                    }
                };

                let tx = tx.clone();
                let router = Arc::clone(&router);
                let on_channel_connect = Arc::clone(&on_channel_connect);
                let socket_status = Arc::clone(&socket_status);
                let req_id = cmd.headers.req_id;

                let request_timeout = config.request_timeout;

                let channel_task_for_cmd = Arc::clone(&channel_task);
                let tx_for_channel = tx_copy.clone();

                let disconnect = disconnect.clone();
                let permit = match Arc::clone(&conn_semaphore).acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => {
                        continue;
                    }
                };

                let socket_status_disc = Arc::clone(&socket_status);
                let on_channel_disconnect = Arc::clone(&on_channel_disconnect);


                inflight.spawn(async move {
                    match tokio::time::timeout(
                        request_timeout,
                        handle_cmd(
                            cmd,
                            socket_status,
                            permit_global,
                            permit,
                            router,
                            Arc::new(move || -> (SocketStatus, Option<u32>) {
                                let (status, mut receiver) = match on_channel_connect() {
                                    Ok(s)  => s,
                                    Err(e)  => {
                                        eprintln!("Error with connecting to channel: {}", e);
                                        return (SocketStatus::Fresh, None);
                                    }
                                };

                                let conn_id = match status {
                                    SocketStatus::Channel(id) => id,
                                    _ => return (status, None)
                                };

                                let tx_channel = tx_for_channel.clone();

                                let relay_task = tokio::spawn(async move {
                                    while let Some(message) = receiver.recv().await {
                                        if tx_channel.send(message).await.is_err() {
                                            break;
                                        }
                                    }
                                });

                                match channel_task_for_cmd.lock() {
                                    Ok(mut slot) => {
                                        if let Some(previous) = slot.replace(relay_task) {
                                            previous.abort();
                                        }
                                    }

                                    Err(_) => {
                                        relay_task.abort();
                                    }
                                }

                                (status, Some(conn_id))
                            }),
                            Arc::new(move || {
                                let status = SocketStatus::from_u64(socket_status_disc.load(Ordering::Relaxed));
                                match status {
                                    SocketStatus::Channel(id) => {
                                        if let Err(e) = on_channel_disconnect(id) {
                                            eprintln!("Error with channel disconnecting: {}", e);
                                        };
                                    },
                                    _ => {

                                    }
                                };
                                disconnect.cancel();
                            })
                        ),
                    ).await {
                        Ok(r) => {
                            if r.is_some()
                                && let Err(e) = tx.send(r.unwrap()).await {
                                    eprintln!("{}", e);
                                }
                        }
                        Err(_) => {
                            if let Err(e) = tx.send(Response::timeout(req_id)).await {
                                eprintln!("{}", e);
                            }
                        }
                    };
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
                    eprintln!("Error with sending to socket: {:?}", e);
                }
                break;
            }
        }
    }

    let status = SocketStatus::from_u64( socket_status.load(Ordering::Relaxed));

    if let SocketStatus::Channel(id) = status {
        if let Err(e) = on_channel_disconnect(id) {
            eprintln!("Error with channel disconnect: {e}");
        }
    }

    inflight.close();
    tokio::select! { _ = inflight.wait() => {}, _ = kill.cancelled() => {} }
    drop(tx);
    drop(conn_permit);
    if let Ok(mut slot) = channel_task.lock() {
        if let Some(task) = slot.take() {
            task.abort();
        }
    }
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
    socket_status: Arc<AtomicU64>,
    permit: OwnedSemaphorePermit,
    status_permit: OwnedSemaphorePermit,
    router: Arc<Router>,
    on_channel_connect: Arc<F>,
    on_channel_disconnect: Arc<T>
) -> Option<Response>
where
    F: Fn() -> (SocketStatus, Option<u32>) + Send + Sync + 'static,
    T: Fn() -> () + Send + Sync + 'static,
{
    let req_id = cmd.headers.req_id;

    if let Err(e) = Router::validate_version(&cmd) {
        return Some(e);
    }

    let status = SocketStatus::from_u64(socket_status.load(Ordering::Relaxed));

    match status {
        SocketStatus::Fresh => match cmd.headers.opcode {
            RequestOpcode::Once => {
                if change_socket_status(SocketStatus::Once, socket_status, status_permit).await.is_err() {
                    return None;
                };
                let response = match AssertUnwindSafe(router.handle(cmd)).catch_unwind().await {
                    Ok(response) => response,
                    Err(_) => Response::internal_error(req_id),
                };

                drop(permit);

                return Some(response);
            }
            RequestOpcode::ChannelConnect => {
                if cmd.route.to_uppercase() == ReservedRoutes::CHANNEL_CONNECT.to_string().to_uppercase() {
                    let (status, id) = on_channel_connect();

                    let conn_id = match id {
                        Some(id) => id,
                        None => {
                            drop(status_permit);
                            drop(permit);

                            return Some(Response::internal_error(req_id));
                        }
                    };

                    if !matches!(status, SocketStatus::Channel(id) if id == conn_id) {
                        drop(status_permit);
                        drop(permit);

                        return Some(Response::internal_error(req_id));
                    }

                    if AssertUnwindSafe(router.on_channel_connect(cmd, conn_id)).catch_unwind().await.is_err() {
                        if change_socket_status(status, socket_status, status_permit).await.is_err() {
                            return None;
                        };

                        on_channel_disconnect();
                        drop(permit);

                        return Some(Response::internal_error(req_id));
                    }

                    if change_socket_status(status, socket_status, status_permit,).await.is_err() {
                        drop(permit);
                        return None;
                    }

                    drop(permit);

                    return Some(Response::channel_connected(req_id));
                } else {
                    drop(status_permit);
                    drop(permit);

                    return Some(Response::bad_request(req_id, Some(Bytes::from("Route must match the opcode"))));
                }
            }
            RequestOpcode::ChannelMessage => {
                drop(status_permit);
                drop(permit);
                return Some(Response::bad_request(
                    req_id,
                    Some(Bytes::from("Channel not connected")),
                ));
            }
        },
        SocketStatus::Once => {
            drop(status_permit);
            match cmd.headers.opcode {
                RequestOpcode::Once => {
                    let response = match AssertUnwindSafe(router.handle(cmd)).catch_unwind().await {
                        Ok(response) => response,
                        Err(_) => Response::internal_error(req_id),
                    };

                    drop(permit);

                    return Some(response);
                }
                _ => {
                    drop(permit);
                    return Some(Response::connection_type_mismatch(req_id));
                }
            }
        },
        SocketStatus::Channel(conn_id) => { 
            drop(status_permit);
            match cmd.headers.opcode {
                RequestOpcode::ChannelConnect => {
                    drop(permit);
                    return Some(Response::conflict(
                        req_id,
                        Bytes::from("Channel already connected"),
                    ));
                }
                RequestOpcode::ChannelMessage => {
                    if cmd.route.to_uppercase() == ReservedRoutes::CHANNEL_DISCONNECT.to_string().to_uppercase() {
                        if AssertUnwindSafe(router.on_channel_disconnect(cmd.clone(), conn_id)).catch_unwind().await.is_err() {
                            return Some(Response::internal_error(req_id));
                        };
                        on_channel_disconnect();
                        return None;
                    }
                    match AssertUnwindSafe(router.handle_channel(cmd, &conn_id)).catch_unwind().await {
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
            }
        },
    };

    None
}

fn writer(
    mut sink: SplitSink<Framed<TcpStream, Framer>, Response>,
    mut rx: Receiver<Response>,
    max_batch: u32,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if let Err(e) = sink.feed(frame).await {
                match e {
                    ProtocolError::MessageTooLarge { message: _, req_id } => {
                        if let Err(err) = sink
                            .feed(Response::internal_error(req_id.unwrap_or_default()))
                            .await
                        {
                            eprintln!("Error sending frame: {}", err);
                        }
                    }
                    ProtocolError::InvalidFrame { message: _, req_id } => {
                        if let Err(err) = sink
                            .feed(Response::internal_error(req_id.unwrap_or_default()))
                            .await
                        {
                            eprintln!("Error sending frame: {}", err);
                        }
                    }
                    ProtocolError::InvalidMessageType { message: _, req_id } => {
                        if let Err(err) = sink
                            .feed(Response::internal_error(req_id.unwrap_or_default()))
                            .await
                        {
                            eprintln!("Error sending frame: {}", err);
                        }
                    }
                    ProtocolError::InvalidUtf8 { req_id } => {
                        if let Err(err) = sink
                            .feed(Response::internal_error(req_id.unwrap_or_default()))
                            .await
                        {
                            eprintln!("Error sending frame: {}", err);
                        }
                    }
                    _ => {
                        eprintln!("Error sending frame: {}", e);
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
                                        eprintln!("Error sending frame: {}", err);
                                    }
                                }
                                ProtocolError::InvalidFrame { message: _, req_id } => {
                                    if let Err(err) = sink
                                        .feed(Response::internal_error(req_id.unwrap_or_default()))
                                        .await
                                    {
                                        eprintln!("Error sending frame: {}", err);
                                    }
                                }
                                ProtocolError::InvalidMessageType { message: _, req_id } => {
                                    if let Err(err) = sink
                                        .feed(Response::internal_error(req_id.unwrap_or_default()))
                                        .await
                                    {
                                        eprintln!("Error sending frame: {}", err);
                                    }
                                }
                                ProtocolError::InvalidUtf8 { req_id } => {
                                    if let Err(err) = sink
                                        .feed(Response::internal_error(req_id.unwrap_or_default()))
                                        .await
                                    {
                                        eprintln!("Error sending frame: {}", err);
                                    }
                                }
                                _ => {
                                    eprintln!("Error sending frame: {}", e);
                                }
                            }
                            return;
                        }
                        count += 1;
                    }
                    Err(_) => break,
                }
            }
            if let Err(e) = sink.flush().await {
                eprintln!("Error flushing sink: {}", e);
                return;
            }
        }
        let _ = sink.close().await;
    })
}

async fn change_socket_status(new: SocketStatus, status: Arc<AtomicU64>, permit: OwnedSemaphorePermit) -> Result<(), ()> {
    status.store(SocketStatus::to_u64(new), Ordering::Relaxed);
    drop(permit);
    Ok(())
}