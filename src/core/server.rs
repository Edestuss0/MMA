use crate::core::commands::Response;
use crate::core::config::Config;
use crate::core::router::Router;
use crate::network::network::SocketStatus;
use crate::network::network::serve_connection;
use std::future::Future;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::{io, panic};
use ahash::HashMap;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

#[derive(Debug)]
pub struct Server {
    config: Config,
    tcp_listener: TcpListener,
    stop: CancellationToken,
    config_receiver: watch::Receiver<Config>,
    config_sender: watch::Sender<Config>,
    channel_hub: Arc<ChannelHub>
}

#[derive(Debug, Default)]
pub struct ChannelHub {
    channels: RwLock<HashMap<u32, mpsc::Sender<Response>>>
}

impl ChannelHub {
    pub fn register(&self, id: u32, sender: mpsc::Sender<Response>) -> Result<(), String> {
        let mut guard = self.channels.write().map_err(|_| String::from("Cannot get access to list of channels"))?;
        guard.insert(id, sender);
        Ok(())
    }

    pub fn disconnect(&self, id: u32) -> Result<(), String> {
        let mut guard = self.channels.write().map_err(|_| String::from("Cannot get access to list of channels"))?;
        guard.remove(&id);
        Ok(())
    }

    pub async fn send(&self, id: u32, item: Response) -> Result<(), String> {
        if id == 0 {
            let channels = {
                let guard = self.channels.read().map_err(|_| String::from("Cannot get access to list of channel"))?;
                guard.values().cloned().collect::<Vec<_>>()
            };

            for sender in channels {
                if sender.send(item.clone()).await.is_err() {
                    return Err(String::from("Error with sending"));
                };
            }
        } else {
            let sender = {
                let guard = self.channels.read().map_err(|_| String::from("Cannot get access to list of channel"))?;
    
                match guard.get(&id).cloned() {
                    Some(s) => s,
                    None => {
                        return Err(String::from("No such channel"));
                    }
                }
            };
    
            if sender.send(item).await.is_err() {
                return Err(String::from("Error with sending"));
            };
        }

        Ok(())
    }

    pub fn try_send(&self, id: u32, item: Response,) -> Result<(), String> {
        if id == 0 {
            let senders = {
                let guard = self.channels.read().map_err(|_| String::from("Cannot get access to list of channel"))?;

                guard.values().cloned().collect::<Vec<_>>()
            };

            let total = senders.len();
            let mut delivered = 0;
            let mut failed = 0;

            for sender in senders {
                match sender.try_send(item.clone()) {
                    Ok(()) => delivered += 1,
                    Err(e) => {
                        failed += 1;
                        eprintln!("Broadcast error: {e}");
                    }
                }
            }

            if failed == 0 {
                return Ok(());
            } else {
                return Err(format!(
                    "Partial broadcast: {delivered}/{total} channels accepted the message"
                ));
            }
        } else {
            let sender = {
                let guard = self.channels.read().map_err(|_| String::from("Cannot get access to list of channel"))?;
    
                match guard.get(&id).cloned() {
                    Some(s) => s,
                    None => {
                        return Err(String::from("No such channel"));
                    }
                }
            };
    
            if sender.try_send(item).is_err() {
                return Err(String::from("Error with sending"));
            };
        }


        Ok(())
    }

}

#[derive(Clone)]
pub struct ServerHandle {
    config_sender: watch::Sender<Config>,
    channel_hub: Arc<ChannelHub>
}

impl ServerHandle {
    pub fn change_config(&self, config: Config) -> Result<(), std::io::Error> {
        Server::validate_config(&config)?;

        self.config_sender.send(config).map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Cannot update config: no active receivers",
            )
        })?;

        Ok(())
    }

    pub async fn send_to_channel(&self, id: u32, item: Response) -> Result<(), String> {
        self.channel_hub.send(id, item).await
    }

    pub fn try_send_to_channel(&self, id: u32, item: Response) -> Result<(), String> {
        self.channel_hub.try_send(id, item)
    }
}

impl Server {
    pub async fn new(config: Config) -> Result<Self, std::io::Error> {
        Self::validate_config(&config)?;
        let listener = TcpListener::bind(config.address).await?;
        Self::from_listener(config, listener)
    }

    pub fn handle(&self) -> ServerHandle {
        ServerHandle {
            channel_hub: Arc::clone(&self.channel_hub),
            config_sender: self.config_sender.clone(),
        }
    }

    pub fn from_listener(
        config: Config,
        tcp_listener: TcpListener,
    ) -> Result<Self, std::io::Error> {
        Self::validate_config(&config)?;
        let (config_sender, config_receiver) = watch::channel::<Config>(config.clone());
        Result::Ok(Self {
            config,
            tcp_listener,
            stop: CancellationToken::new(),
            config_receiver,
            config_sender,
            channel_hub: Arc::new(ChannelHub::default())
        })
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.stop.clone()
    }

    pub fn shutdown_server(&self) {
        self.stop.cancel();
    }

    fn validate_config(config: &Config) -> Result<(), io::Error> {
        if let Err(e) = config.validate() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, e));
        }
        Result::Ok(())
    }
    pub fn local_addr(&self) -> Result<std::net::SocketAddr, std::io::Error> {
        self.tcp_listener.local_addr()
    }

    pub async fn run(&mut self, router: Router) -> Result<(), std::io::Error> {
        println!("Server listen on {} now", self.config.address);
        self.run_until(router, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
    }

    pub async fn run_until<F: Future<Output = ()>>(
        &mut self,
        router: Router,
        shutdown: F,
    ) -> Result<(), std::io::Error> {
        let router = Arc::new(router);
        let semaphore = Arc::new(Semaphore::new(self.config.max_in_flight as usize));
        let stop = self.stop.clone();
        let kill = CancellationToken::new();
        let conns = TaskTracker::new();
        let mut result = Result::Ok(());
        tokio::pin!(shutdown);

        let conns_semaphore = Arc::new(Semaphore::new(self.config.max_connections as usize));

        let current_channel_connection_id: Arc<AtomicU32> = Arc::new(AtomicU32::new(1));

        loop {
            let conns_semaphore = Arc::clone(&conns_semaphore);
            let current_channel_connection_id = Arc::clone(&current_channel_connection_id);
            let mut socket = tokio::select! {
                _ = &mut shutdown => break,
                _ = stop.cancelled() => break,
                acc = self.tcp_listener.accept() => match acc {
                    Result::Ok((socket, _)) => socket,
                    Result::Err(e) => { result = Err(e); continue; }
                },
            };
            let permit = match conns_semaphore.try_acquire_owned() {
                Result::Ok(p) => p,
                Err(_) => {
                    socket.shutdown().await?;
                    continue;
                }
            };

            let _ = socket.set_nodelay(true);

            let channel_hub_disc = Arc::clone(&self.channel_hub);
            let channel_hub = Arc::clone(&self.channel_hub);

            conns.spawn(serve_connection(
                socket,
                Arc::clone(&router),
                self.config_receiver.clone(),
                Arc::clone(&semaphore),
                stop.clone(),
                kill.clone(),
                permit,
                move || {
                    let conn_id = current_channel_connection_id.fetch_add(1, Ordering::Relaxed);
                    let (sender, receiver) = mpsc::channel::<Response>(256);
                    if let Err(e) = channel_hub.register(conn_id, sender) {
                        return Err(e);
                    }
                    Ok((SocketStatus::Channel(conn_id), receiver))
                },
                move |id| {
                    channel_hub_disc.disconnect(id)
                },
            ));
        }

        stop.cancel();
        conns.close();
        if tokio::time::timeout(self.config.shutdown_grace, conns.wait())
            .await
            .is_err()
        {
            eprintln!("graceful shutdown: grace period expired, killing connections");
            kill.cancel();
            conns.wait().await;
        }
        result
    }
}
