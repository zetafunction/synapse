use std::io::{self, Cursor, Read, Write};
use std::net::{SocketAddr, UdpSocket};
use std::time;

use byteorder::{BigEndian, ByteOrder, ReadBytesExt, WriteBytesExt};
use rand::random;

use crate::tracker::{Announce, Error, Event, Response, Result, TrackerResponse, dns};
use crate::util::{FHashMap, UHashMap, bytes_to_addr};
use crate::{CONFIG, PEER_ID};

// We're not going to bother with backoff, if the tracker/network aren't working now
// the torrent can just resend a request later.
const TIMEOUT_MS: u64 = 15_000;
const RETRANS_MS: u64 = 5_000;
const MAGIC_NUM: u64 = 0x417_2710_1980;

pub struct Handler {
    id: usize,
    sock: UdpSocket,
    connections: UHashMap<Connection>,
    transactions: FHashMap<u32, usize>,
    conn_count: usize,
    buf: Vec<u8>,
}

struct Connection {
    torrent: usize,
    last_updated: time::Instant,
    last_retrans: time::Instant,
    state: State,
    announce: Announce,
}

enum State {
    ResolvingDNS { port: u16 },
    Connecting { addr: SocketAddr, data: [u8; 16] },
    Announcing { addr: SocketAddr, data: [u8; 98] },
}

impl Handler {
    pub fn new(reg: &amy::Registrar) -> io::Result<Handler> {
        let port = CONFIG.trk.port;
        let sock = UdpSocket::bind(("0.0.0.0", port))?;
        sock.set_nonblocking(true)?;
        let id = reg.register(&sock, amy::Event::Read)?;
        Ok(Handler {
            id,
            sock,
            connections: UHashMap::default(),
            transactions: FHashMap::default(),
            conn_count: 0,
            buf: vec![0u8; 350],
        })
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn complete(&self) -> bool {
        self.connections.is_empty()
    }

    pub fn active_requests(&self) -> usize {
        self.connections.len()
    }

    pub fn contains(&self, id: usize) -> bool {
        self.connections.contains_key(&id)
    }

    pub fn new_announce(&mut self, req: Announce, dns: &mut dns::Resolver) -> Result<()> {
        let url = req.url.clone();
        debug!("Received a new announce req for {:?}", url);
        let host = url
            .host_str()
            .ok_or_else(|| Error::UrlNoHost(url.as_ref().clone().into()))?;
        let port = url
            .port()
            .ok_or_else(|| Error::UrlNoPort(url.as_ref().clone().into()))?;

        let id = self.new_conn();
        self.connections.insert(
            id,
            Connection {
                torrent: req.id,
                last_updated: time::Instant::now(),
                last_retrans: time::Instant::now(),
                state: State::ResolvingDNS { port },
                announce: req,
            },
        );
        debug!("Dispatching DNS req for {:?}, url: {:?}", id, host);
        if let Some(ip) = dns.new_query(id, host).map_err(Error::DnsIo)? {
            debug!("Using cached DNS response");
            let res = self.dns_resolved(dns::QueryResponse { id, res: Ok(ip) });
            if res.is_some() {
                return Err(Error::Connection);
            }
        }
        Ok(())
    }

    pub fn dns_resolved(&mut self, resp: dns::QueryResponse) -> Option<Response> {
        let id = resp.id;
        let mut success = false;
        debug!("Received a DNS resp for {:?}", id);
        let resp = if let Some(conn) = self.connections.get_mut(&id) {
            match conn.state {
                State::ResolvingDNS { port } => {
                    conn.last_updated = time::Instant::now();
                    let tid = random::<u32>();
                    let mut data = [0u8; 16];
                    {
                        let mut connect_req = Cursor::new(&mut data[..]);
                        connect_req.write_u64::<BigEndian>(MAGIC_NUM).unwrap();
                        connect_req.write_u32::<BigEndian>(0).unwrap();
                        connect_req.write_u32::<BigEndian>(tid).unwrap();
                    }
                    match resp.res {
                        Ok(ip) => {
                            success = true;
                            conn.state = State::Connecting {
                                addr: SocketAddr::new(ip, port),
                                data,
                            };
                            self.transactions.insert(tid, id);
                            None
                        }
                        Err(e) => Some(Response::Tracker {
                            tid: conn.torrent,
                            url: conn.announce.url.clone(),
                            resp: Err(e),
                        }),
                    }
                }
                _ => None,
            }
        } else {
            None
        };
        if resp.is_some() {
            self.connections.remove(&id);
            resp
        } else if success {
            self.send_data(id)
        } else {
            None
        }
    }

    pub fn readable(&mut self) -> Vec<Response> {
        let mut resps = Vec::new();
        while let Ok((v, _)) = self.sock.recv_from(&mut self.buf[..]) {
            let action = BigEndian::read_u32(&self.buf[0..4]);
            match action {
                0 if v == 16 => {
                    if let Some(r) = self.process_connect() {
                        resps.push(r);
                    }
                }
                1 if v >= 20 => {
                    if let Some(r) = self.process_announce(v) {
                        resps.push(r);
                    }
                }
                3 if v >= 8 => {
                    if let Some(r) = self.process_error(v) {
                        resps.push(r);
                    }
                }
                _ => {
                    // TODO: Is this worth logging/reporting?
                    debug!("Received invalid response from tracker!");
                }
            }
        }
        resps
    }

    pub fn tick(&mut self) -> Vec<Response> {
        let mut resps = Vec::new();
        let mut retrans = Vec::new();
        {
            self.connections.retain(|id, conn| {
                if conn.last_updated.elapsed() > time::Duration::from_millis(TIMEOUT_MS) {
                    resps.push(Response::Tracker {
                        tid: conn.torrent,
                        url: conn.announce.url.clone(),
                        resp: Err(Error::Timeout),
                    });
                    debug!("Announce {:?} timed out", id);
                    false
                } else {
                    if conn.last_retrans.elapsed() > time::Duration::from_millis(RETRANS_MS) {
                        debug!("Retransmiting req {:?}", id);
                        retrans.push(*id);
                    }
                    true
                }
            });

            let c = &self.connections;
            self.transactions.retain(|_, id| c.contains_key(id));
        }

        for id in retrans {
            if let Some(r) = self.send_data(id) {
                resps.push(r)
            }
        }
        resps
    }

    fn process_connect(&mut self) -> Option<Response> {
        let (transaction_id, connection_id) = {
            let mut connect_resp = Cursor::new(&self.buf[4..16]);
            let tid = connect_resp.read_u32::<BigEndian>().unwrap();
            let cid = connect_resp.read_u64::<BigEndian>().unwrap();
            (tid, cid)
        };

        let id = self.transactions.remove(&transaction_id)?;

        let mut data = [0u8; 98];
        {
            let conn = self.connections.get_mut(&id)?;
            let addr = match conn.state {
                State::Connecting { addr, .. } => addr,
                _ => return None,
            };

            {
                let mut announce_req = Cursor::new(&mut data[..]);
                announce_req.write_u64::<BigEndian>(connection_id).unwrap();
                // announce action
                announce_req.write_u32::<BigEndian>(1).unwrap();

                let tid = random::<u32>();
                announce_req.write_u32::<BigEndian>(tid).unwrap();
                self.transactions.insert(tid, id);

                announce_req.write_all(&conn.announce.hash).unwrap();
                announce_req.write_all(&PEER_ID[..]).unwrap();
                announce_req
                    .write_u64::<BigEndian>(conn.announce.downloaded)
                    .unwrap();
                announce_req
                    .write_u64::<BigEndian>(conn.announce.left)
                    .unwrap();
                announce_req
                    .write_u64::<BigEndian>(conn.announce.uploaded)
                    .unwrap();
                match conn.announce.event {
                    Some(Event::Started) => {
                        announce_req.write_u32::<BigEndian>(2).unwrap();
                    }
                    Some(Event::Stopped) => {
                        announce_req.write_u32::<BigEndian>(3).unwrap();
                    }
                    Some(Event::Completed) => {
                        announce_req.write_u32::<BigEndian>(1).unwrap();
                    }
                    None => {
                        announce_req.write_u32::<BigEndian>(0).unwrap();
                    }
                }

                // IP
                announce_req.write_u32::<BigEndian>(0).unwrap();
                // Key - TODO: randomly generate this
                announce_req.write_u32::<BigEndian>(0xFFFF_00BA).unwrap();
                // Num want
                let nw = conn.announce.num_want.map(i32::from).unwrap_or(-1);
                announce_req.write_i32::<BigEndian>(nw).unwrap();
                // port
                announce_req
                    .write_u16::<BigEndian>(conn.announce.port)
                    .unwrap();
            }
            conn.state = State::Announcing { addr, data };
            conn.last_updated = time::Instant::now();
        }
        self.send_data(id)
    }

    fn process_announce(&mut self, len: usize) -> Option<Response> {
        let mut announce_resp = Cursor::new(&self.buf[4..len]);
        let mut resp = TrackerResponse::empty();
        let transaction_id = announce_resp.read_u32::<BigEndian>().unwrap();

        let id = self.transactions.remove(&transaction_id)?;

        let conn = self.connections.remove(&id)?;

        resp.interval = announce_resp.read_u32::<BigEndian>().unwrap();
        resp.leechers = announce_resp.read_u32::<BigEndian>().unwrap();
        resp.seeders = announce_resp.read_u32::<BigEndian>().unwrap();
        if len > 20 {
            let pos = announce_resp.position() as usize;
            for p in announce_resp.get_ref()[pos..].chunks(6) {
                resp.peers.push(bytes_to_addr(p));
            }
        }
        Some(Response::Tracker {
            tid: conn.torrent,
            url: conn.announce.url,
            resp: Ok(resp),
        })
    }

    fn process_error(&mut self, len: usize) -> Option<Response> {
        let mut s = String::new();
        let mut connect_resp = Cursor::new(&self.buf[4..len]);
        let transaction_id = connect_resp.read_u32::<BigEndian>().unwrap();

        let id = self.transactions.remove(&transaction_id)?;

        let conn = self.connections.remove(&id)?;

        match connect_resp.read_to_string(&mut s) {
            Ok(_) => Some(Response::Tracker {
                tid: conn.torrent,
                url: conn.announce.url,
                resp: Err(Error::TrackerError(s)),
            }),
            Err(e) => Some(Response::Tracker {
                tid: conn.torrent,
                url: conn.announce.url,
                resp: Err(Error::UdpResponseInvalid(e)),
            }),
        }
    }

    fn new_conn(&mut self) -> usize {
        let c = self.conn_count;
        self.conn_count = self.conn_count.wrapping_add(1);
        c
    }

    fn send_data(&mut self, id: usize) -> Option<Response> {
        let tid;
        let res = {
            let conn = self.connections.get_mut(&id).unwrap();
            tid = conn.torrent;
            // If this actually blocks, something is really fucked(prob with the NIC)
            // and i dont think we need to care
            match conn.state {
                State::Connecting { ref addr, ref data } => {
                    conn.last_retrans = time::Instant::now();
                    self.sock.send_to(data, addr).map_err(Error::SendTo)
                }
                State::Announcing { ref addr, ref data } => {
                    conn.last_retrans = time::Instant::now();
                    self.sock.send_to(data, addr).map_err(Error::SendTo)
                }
                _ => Ok(0),
            }
        };

        match res {
            Err(e) => {
                let url = self.connections.remove(&id).unwrap().announce.url;
                Some(Response::Tracker {
                    tid,
                    url,
                    resp: Err(e),
                })
            }
            Ok(_) => None,
        }
    }
}
