use std::sync::Arc;
use tokio::sync::watch;

use crate::{
    config::config::Config,
    core::{channel_hub::ChannelHub, commands::Response, server::Server},
};

#[derive(Clone)]
pub struct ServerHandle {
    config_sender: watch::Sender<Config>,
    channel_hub: Arc<ChannelHub>,
}

impl ServerHandle {
    pub fn new(config_sender: watch::Sender<Config>, channel_hub: Arc<ChannelHub>) -> Self {
        Self {
            config_sender,
            channel_hub,
        }
    }

    fn current_config(&self) -> Config {
        let receiver = self.config_sender.subscribe();
        let current = receiver.borrow();
        (*current).clone()
    }

    pub fn change_config(&self, config: Config) -> Result<(), std::io::Error> {
        Server::validate_config(&config)?;

        let current = self.current_config();
        let tls_changed = current.tls_config != config.tls_config;

        if tls_changed {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "TLS settings cannot be changed while server is running",
            ));
        }

        self.config_sender.send(config).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
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
