use anyhow::{bail, Result};
use sstream::SStream;
use url::Url;
use ws::protocol::Message as WSMessage;

use crate::rpc::message::{CMessage, SMessage, Version};

const OS_IN_PROGRESS_ERROR: i32 = 36;

pub struct Client {
    ws: ws::WebSocket<SStream>,
    version: Version,
    serial: u64,
}

impl Client {
    pub fn new(url: Url) -> Result<Client> {
        if !url.has_host() {
            bail!("Invalid websocket URL {}!", url);
        }
        for addr in url.socket_addrs(|| None)? {
            let mut stream = match url.scheme() {
                "ws" => {
                    if addr.is_ipv4() {
                        SStream::new_v4(None)
                    } else {
                        SStream::new_v6(None)
                    }
                }
                "wss" => {
                    if addr.is_ipv4() {
                        SStream::new_v4(Some(url.host_str().unwrap().to_owned()))
                    } else {
                        SStream::new_v6(Some(url.host_str().unwrap().to_owned()))
                    }
                }
                _ => bail!("Cannot create client for non-websocket URL {}", url),
            }?;
            let connect_err = stream.connect(addr);
            match connect_err {
                Err(e) if e.raw_os_error() == Some(OS_IN_PROGRESS_ERROR) => {}
                other => other?,
            };
            stream.get_stream().set_nonblocking(false)?;
            let config = ws::protocol::WebSocketConfig::default()
                .max_message_size(None)
                .max_frame_size(None);
            if let Ok((client, _response)) =
                ws::client::client_with_config(url.as_str(), stream, Some(config))
            {
                let mut c = Client {
                    ws: client,
                    serial: 0,
                    version: Version { major: 0, minor: 0 },
                };
                if let SMessage::RpcVersion(v) = c.recv()? {
                    c.version = v;
                    return Ok(c);
                } else {
                    bail!("Expected a version message on start!");
                }
            }
        }
        bail!("Could not connect to provided URL {}!", url);
    }

    pub fn version(&self) -> &Version {
        &self.version
    }

    pub fn next_serial(&mut self) -> u64 {
        self.serial += 1;
        self.serial - 1
    }

    pub fn send(&mut self, msg: CMessage) -> Result<()> {
        let msg_data = serde_json::to_string(&msg)?;
        self.ws.send(WSMessage::Text(msg_data.into()))?;
        Ok(())
    }

    pub fn recv(&mut self) -> Result<SMessage<'static>> {
        loop {
            match self.ws.read() {
                Ok(WSMessage::Text(s)) => {
                    return Ok(serde_json::from_str(&s)?);
                }
                Ok(WSMessage::Ping(p)) => {
                    self.ws.send(WSMessage::Pong(p))?;
                }
                Err(e) => Err(e)?,
                _ => {}
            };
        }
    }

    pub fn rr(&mut self, msg: CMessage) -> Result<SMessage<'static>> {
        self.send(msg)?;
        self.recv()
    }
}
