use std::{collections::HashMap, sync::RwLock};

use tokio::sync::mpsc;

use crate::core::commands::Response;

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

