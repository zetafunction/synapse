use std::io::{self, Write};
use std::{mem, result, str, time};

use sstream::SStream;
use url::Url;

use super::proto::message::{SMessage, Version};
use super::proto::ws::{Frame, Message, Opcode};
use super::reader::Reader;
use super::writer::Writer;
use super::{ErrorKind, Result, ResultExt};
use super::{EMPTY_HTTP_RESP, UNAUTH_HTTP_RESP};
use crate::util::{aread, sha1_hash, IOR};
use crate::{CONFIG, DL_TOKEN};

pub struct Client {
    pub conn: SStream,
    r: Reader,
    w: Writer,
    buf: FragBuf,
    last_action: time::Instant,
}

pub struct Incoming {
    pub conn: SStream,
    key: Option<String>,
    buf: [u8; 1024],
    pos: usize,
    last_action: time::Instant,
}

pub enum IncomingStatus {
    Incomplete,
    Upgrade,
    Transfer { data: Vec<u8>, token: String },
    DL { id: String, range: Option<String> },
}

enum FragBuf {
    None,
    Text(Vec<u8>),
    Binary(Vec<u8>),
}

const CONN_TIMEOUT: u64 = 20;
const CONN_PING: u64 = 15;

impl Client {
    pub fn read(&mut self) -> Result<Option<Frame>> {
        self.last_action = time::Instant::now();
        loop {
            match self.read_frame()? {
                Ok(f) => return Ok(Some(f)),
                Err(true) => return Ok(None),
                Err(false) => continue,
            }
        }
    }

    fn read_frame(&mut self) -> Result<result::Result<Frame, bool>> {
        let m = match self.r.read(&mut self.conn).chain_err(|| ErrorKind::IO)? {
            Some(m) => m,
            None => return Ok(Err(true)),
        };
        if m.opcode().is_control() && m.len > 125 {
            return Err(ErrorKind::BadPayload("Control frame too long!").into());
        }
        if m.opcode().is_control() && !m.fin() {
            return Err(ErrorKind::BadPayload("Control frame must not be fragmented!").into());
        }
        if m.opcode().is_other() {
            return Err(ErrorKind::BadPayload("Non standard opcodes unsupported!").into());
        }
        if m.extensions() {
            return Err(ErrorKind::BadPayload("Connection should not contain RSV bits!").into());
        }
        match m.opcode() {
            Opcode::Close => {
                self.send_msg(Message::close())?;
                return Err(ErrorKind::Complete.into());
            }
            Opcode::Text | Opcode::Binary | Opcode::Continuation => {
                if let Some(f) = self.buf.process(m)? {
                    #[cfg(feature = "autobahn")]
                    self.send(f)?;
                    #[cfg(not(feature = "autobahn"))]
                    return Ok(Ok(f));
                }
            }
            Opcode::Ping => {
                self.send_msg(Message::pong(m.data))?;
            }
            Opcode::Pong => {
                self.last_action = time::Instant::now();
            }
            _ => {}
        }
        Ok(Err(false))
    }

    pub fn write(&mut self) -> Result<()> {
        self.w.write(&mut self.conn).chain_err(|| ErrorKind::IO)
    }

    pub fn send(&mut self, f: Frame) -> Result<()> {
        self.send_msg(f.into())
    }

    fn send_msg(&mut self, msg: Message) -> Result<()> {
        self.w.enqueue(msg);
        self.write()
    }

    pub fn timed_out(&mut self) -> bool {
        if self.last_action.elapsed().as_secs() > CONN_TIMEOUT {
            return true;
        }
        self.last_action.elapsed().as_secs() > CONN_PING
            && self.send_msg(Message::ping(vec![0xDE, 0xAD])).is_err()
    }
}

impl Into<SStream> for Client {
    fn into(self) -> SStream {
        self.conn
    }
}

impl Into<Client> for Incoming {
    fn into(mut self) -> Client {
        let magic = self.key.unwrap() + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
        let digest = sha1_hash(magic.as_bytes());
        let accept = base64::encode(digest.as_ref());
        let lines = [
            format!("HTTP/1.1 101 Switching Protocols"),
            format!("Connection: upgrade"),
            format!("Upgrade: websocket"),
            format!("Sec-WebSocket-Accept: {}", accept),
        ];
        let data = lines.join("\r\n") + "\r\n\r\n";
        // Ignore error, it'll pop up again anyways
        self.conn.write(data.as_bytes()).ok();

        let mut c = Client {
            r: Reader::new(),
            w: Writer::new(),
            buf: FragBuf::None,
            conn: self.conn,
            last_action: time::Instant::now(),
        };

        c.send(Frame::Text(
            serde_json::to_string(&SMessage::RpcVersion(Version::current())).unwrap(),
        ))
        .ok();
        c
    }
}

impl Into<SStream> for Incoming {
    fn into(self) -> SStream {
        self.conn
    }
}

impl Incoming {
    pub fn new(conn: SStream) -> Incoming {
        Incoming {
            conn,
            buf: [0; 1024],
            pos: 0,
            last_action: time::Instant::now(),
            key: None,
        }
    }

    /// Result indicates if the Incoming connection is
    /// valid to be upgraded into a Client
    pub fn readable(&mut self) -> io::Result<IncomingStatus> {
        self.last_action = time::Instant::now();
        loop {
            match aread(&mut self.buf[self.pos..], &mut self.conn) {
                // TODO: Consider more
                IOR::Complete => {
                    self.pos = self.buf.len();
                    if let Some(r) = self.process_incoming()? {
                        return Ok(r);
                    } else {
                        return Err(io::ErrorKind::UnexpectedEof.into());
                    }
                }
                IOR::Incomplete(a) => {
                    self.pos += a;
                    if let Some(r) = self.process_incoming()? {
                        return Ok(r);
                    }
                }
                IOR::Blocked => return Ok(IncomingStatus::Incomplete),
                IOR::EOF => return Err(io::ErrorKind::UnexpectedEof.into()),
                IOR::Err(e) => return Err(e),
            }
        }
    }

    pub fn timed_out(&self) -> bool {
        self.last_action.elapsed().as_secs() > CONN_TIMEOUT
    }

    fn process_incoming(&mut self) -> io::Result<Option<IncomingStatus>> {
        let mut headers = [httparse::EMPTY_HEADER; 24];
        let mut req = httparse::Request::new(&mut headers);
        match req.parse(&self.buf[..self.pos]) {
            Ok(httparse::Status::Partial) => Ok(None),
            Ok(httparse::Status::Complete(idx)) => {
                if req.method == Some("HEAD") {
                    self.conn.write(&EMPTY_HTTP_RESP).ok();
                    return Err(io::ErrorKind::InvalidData.into());
                }
                match validate_upgrade(&req) {
                    Ok(k) => {
                        self.key = Some(k);
                        return Ok(Some(IncomingStatus::Upgrade));
                    }
                    Err(true) => {
                        self.conn.write(&UNAUTH_HTTP_RESP).ok();
                        return Err(io::ErrorKind::InvalidData.into());
                    }
                    Err(false) => {}
                }
                if let Some(token) = validate_tx(&req) {
                    Ok(Some(IncomingStatus::Transfer {
                        data: self.buf[idx..self.pos].to_owned(),
                        token,
                    }))
                } else if let Some((id, range)) = validate_dl(&req) {
                    Ok(Some(IncomingStatus::DL { id, range }))
                } else {
                    // Ignore error, we're DCing anyways
                    self.conn.write(&EMPTY_HTTP_RESP).ok();
                    Err(io::ErrorKind::InvalidData.into())
                }
            }
            Err(_) => Err(io::ErrorKind::InvalidData.into()),
        }
    }
}

impl FragBuf {
    fn process(&mut self, msg: Message) -> Result<Option<Frame>> {
        let fin = msg.fin();
        let s = mem::replace(self, FragBuf::None);
        *self = match (s, msg.opcode()) {
            (FragBuf::None, Opcode::Text) => FragBuf::Text(msg.data),
            (FragBuf::None, Opcode::Binary) => FragBuf::Binary(msg.data),
            (FragBuf::None, Opcode::Continuation) => {
                return Err(ErrorKind::BadPayload("Invalid continuation frame").into());
            }
            (FragBuf::Text(mut b), Opcode::Continuation) => {
                b.extend(msg.data.into_iter());
                FragBuf::Text(b)
            }
            (FragBuf::Binary(mut b), Opcode::Continuation) => {
                b.extend(msg.data.into_iter());
                FragBuf::Binary(b)
            }
            (FragBuf::Text(_), Opcode::Text)
            | (FragBuf::Text(_), Opcode::Binary)
            | (FragBuf::Binary(_), Opcode::Text)
            | (FragBuf::Binary(_), Opcode::Binary) => {
                return Err(ErrorKind::BadPayload("Expected continuation of data frame").into());
            }
            _ => return Ok(None),
        };
        if fin {
            match mem::replace(self, FragBuf::None) {
                FragBuf::Text(b) => {
                    let t = String::from_utf8(b)
                        .chain_err(|| ErrorKind::BadPayload("Invalid Utf8 in text!"))?;
                    Ok(Some(Frame::Text(t)))
                }
                FragBuf::Binary(b) => Ok(Some(Frame::Binary(b))),
                FragBuf::None => unreachable!(),
            }
        } else {
            Ok(None)
        }
    }
}

fn validate_dl(req: &httparse::Request<'_, '_>) -> Option<(String, Option<String>)> {
    req.path
        .and_then(|path| Url::parse(&format!("http://localhost{}", path)).ok())
        .and_then(|url| {
            let id = if url.path().contains("/dl/") {
                url.path_segments().unwrap().last().map(|v| v.to_owned())
            } else {
                return None;
            };
            if CONFIG.rpc.auth {
                let pw = url
                    .query_pairs()
                    .find(|&(ref k, _)| k == "token")
                    .map(|(_, v)| format!("{}", v))
                    .and_then(|p| base64::decode(&p).ok())
                    .map(|p| {
                        p.as_ref()
                            == sha1_hash(
                                format!(
                                    "{}{}",
                                    id.as_ref().map(|s| s.as_str()).unwrap_or(""),
                                    *DL_TOKEN
                                )
                                .as_bytes(),
                            )
                    })
                    .unwrap_or(false);
                if !pw {
                    return None;
                }
            }
            id
        })
        .map(|id| {
            let range = req
                .headers
                .iter()
                .find(|header| header.name.to_lowercase() == "range")
                .and_then(|header| str::from_utf8(header.value).ok())
                .map(str::to_owned);
            (id, range)
        })
}

// TODO: We're not really checking HTTP semantics here, might be worth
// considering.
fn validate_tx(req: &httparse::Request<'_, '_>) -> Option<String> {
    for header in req.headers.iter() {
        if header.name.to_lowercase() == "authorization" {
            return str::from_utf8(header.value).ok().and_then(|v| {
                if v.to_lowercase().starts_with("bearer ") {
                    let (_, tok) = v.split_at(7);
                    Some(tok.to_owned())
                } else {
                    None
                }
            });
        }
    }
    None
}

fn validate_upgrade(req: &httparse::Request<'_, '_>) -> result::Result<String, bool> {
    if !req.method.map(|m| m == "GET").unwrap_or(false) {
        return Err(false);
    }

    let mut upgrade = None;
    let mut key = None;
    let mut version = None;

    for header in req.headers.iter() {
        if header.name.to_lowercase() == "upgrade" {
            upgrade = str::from_utf8(header.value).ok();
        }
        if header.name.to_lowercase() == "sec-websocket-key" {
            key = str::from_utf8(header.value).ok();
        }
        if header.name.to_lowercase() == "sec-websocket-version" {
            version = str::from_utf8(header.value).ok();
        }
    }

    if upgrade.map(|s| s.to_lowercase()) != Some("websocket".to_owned()) {
        return Err(false);
    }

    if version != Some("13") {
        return Err(false);
    }

    if CONFIG.rpc.auth {
        let auth = req
            .path
            .and_then(|path| Url::parse(&format!("http://localhost{}", path)).ok())
            .and_then(|url| {
                url.query_pairs()
                    .find(|&(ref k, _)| k == "password")
                    .map(|(_, v)| format!("{}", v))
                    .map(|p| p == CONFIG.rpc.password)
            })
            .or_else(|| {
                req.headers
                    .iter()
                    .find(|header| header.name.to_lowercase() == "authorization")
                    .and_then(|header| str::from_utf8(header.value).ok())
                    .and_then(|value| {
                        if value.to_lowercase().starts_with("basic ") {
                            let (_, auth) = value.split_at(6);
                            Some(auth)
                        } else {
                            None
                        }
                    })
                    .and_then(|auth| base64::decode(auth).ok())
                    .and_then(|auth| String::from_utf8(auth).ok())
                    .and_then(|auth| {
                        auth.split_terminator(':')
                            .last()
                            .map(|password| password == CONFIG.rpc.password)
                    })
            })
            .unwrap_or(false);
        if !auth {
            return Err(true);
        }
    }

    if let Some(k) = key {
        Ok(k.to_owned())
    } else {
        Err(false)
    }
}
