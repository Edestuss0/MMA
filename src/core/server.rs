use crate::core::channel_hub::ChannelHub;
use crate::core::commands::Response;
use crate::config::config::Config;
use crate::core::handle::ServerHandle;
use crate::core::router::Router;
use crate::core::commands::SocketStatus;
use crate::core::tls;
use crate::network::network::AsyncConnection;
use crate::network::network::serve_connection;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::{io, panic};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub struct Server {
    config: Config,
    tcp_listener: TcpListener,
    stop: CancellationToken,
    config_receiver: watch::Receiver<Config>,
    config_sender: watch::Sender<Config>,
    channel_hub: Arc<ChannelHub>,
    tls_acceptor: Option<TlsAcceptor>
}

impl Server {
    pub async fn new(config: Config) -> Result<Self, std::io::Error> {
        Self::validate_config(&config)?;
        let listener = TcpListener::bind(config.address).await?;
        Self::from_listener(config, listener)
    }

    pub fn handle(&self) -> ServerHandle { ServerHandle::new(self.config_sender.clone(), Arc::clone(&self.channel_hub)) }

    pub fn from_listener(
        config: Config,
        tcp_listener: TcpListener,
    ) -> Result<Self, std::io::Error> {
        Self::validate_config(&config)?;
        let (config_sender, config_receiver) = watch::channel::<Config>(config.clone());
        let tls_acceptor = if config.tls_enable && config.tls_certificate_path.is_some() && config.tls_key_path.is_some() && config.tls_handshake_timeout.is_some() {
            match tls::create_tls_acceptor(config.tls_certificate_path.clone().unwrap(), config.tls_key_path.clone().unwrap()) {
                Ok(a)  => Some(a),
                Err(e) => {
                    eprintln!("Invalid TLS config: {}", e);
                    None
                }
            }
        } else { None };
        Result::Ok(Self {
            config,
            tcp_listener,
            stop: CancellationToken::new(),
            config_receiver,
            config_sender,
            channel_hub: Arc::new(ChannelHub::default()),
            tls_acceptor: tls_acceptor
        })
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.stop.clone()
    }

    pub fn shutdown_server(&self) {
        self.stop.cancel();
    }

    pub fn validate_config(config: &Config) -> Result<(), io::Error> {
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

        let tls_acceptor = self.tls_acceptor.clone();
        let tls_handshake_timeout = self.config.tls_handshake_timeout;

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

            let router = Arc::clone(&router);
            let config_receiver = self.config_receiver.clone();
            let semaphore = Arc::clone(&semaphore);
            let stop = stop.clone();
            let kill = kill.clone();
            let tls_acceptor = tls_acceptor.clone();

            conns.spawn(async move {
                let stream: Box<dyn AsyncConnection> = match (tls_acceptor, tls_handshake_timeout) {
                    (Some(ta), Some(tht)) => {
                        match tokio::time::timeout(tht, ta.accept(socket)).await {
                            Ok(Ok(ts)) => {
                                Box::new(ts)
                            },
                            Ok(Err(e)) => {
                                eprintln!("TLS Handshake failed: {}", e);
                                return;
                            },
                            Err(_) => {
                                eprintln!("TLS Handshake timeout");
                                return;
                            }
                        }
                    },
                    _ => {
                        Box::new(socket)
                    }
                };

                serve_connection(
                    stream,
                    router,
                    config_receiver,
                    semaphore,
                    stop,
                    kill,
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
                ).await;
            });
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
