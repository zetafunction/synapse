mod reader;
mod writer;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{io, mem};

use sstream::SStream;
use url::Url;

use self::reader::{ReadRes, Reader};
use self::writer::Writer;
use crate::tracker::{self, Announce, Error, Response, Result, TrackerResponse, dns};
use crate::util::{UHashMap, http};
use crate::{PEER_ID, bencode};

const TIMEOUT_MS: u64 = 5_000;

pub struct Handler {
    reg: amy::Registrar,
    connections: UHashMap<Tracker>,
}

enum Event {
    DNSResolved(dns::QueryResponse),
    Readable,
    Writable,
}

struct Tracker {
    torrent: usize,
    url: Arc<Url>,
    last_updated: Instant,
    redirect: bool,
    state: TrackerState,
}

enum TrackerState {
    Error,
    ResolvingDNS {
        sock: SStream,
        req: Vec<u8>,
        port: u16,
    },
    Writing {
        sock: SStream,
        writer: Writer,
    },
    Reading {
        sock: SStream,
        reader: Reader,
    },
    Redirect(String),
    Complete(TrackerResponse),
}

enum HTTPRes {
    None,
    Redirect(String),
    Complete(TrackerResponse),
}

impl TrackerState {
    fn new(sock: SStream, req: Vec<u8>, port: u16) -> TrackerState {
        TrackerState::ResolvingDNS { sock, req, port }
    }

    fn handle(&mut self, event: Event) -> Result<HTTPRes> {
        let s = mem::replace(self, TrackerState::Error);
        match s.next(event)? {
            TrackerState::Complete(r) => Ok(HTTPRes::Complete(r)),
            TrackerState::Redirect(l) => Ok(HTTPRes::Redirect(l)),
            n => {
                *self = n;
                Ok(HTTPRes::None)
            }
        }
    }

    fn next(self, event: Event) -> Result<TrackerState> {
        match (self, event) {
            (
                TrackerState::ResolvingDNS {
                    mut sock,
                    req,
                    port,
                },
                Event::DNSResolved(r),
            ) => {
                let addr = SocketAddr::new(r.res?, port);
                sock.connect(addr).map_err(Error::Connect)?;
                Ok(TrackerState::Writing {
                    sock,
                    writer: Writer::new(req),
                }
                .next(Event::Writable)?
                .next(Event::Readable)?)
            }
            (
                TrackerState::Writing {
                    mut sock,
                    mut writer,
                },
                _,
            ) => match writer.writable(&mut sock)? {
                Some(()) => {
                    debug!("Tracker write completed, beginning read");
                    let r = Reader::new();
                    Ok(TrackerState::Reading { sock, reader: r }.next(Event::Readable)?)
                }
                None => Ok(TrackerState::Writing { sock, writer }),
            },
            (
                TrackerState::Reading {
                    mut sock,
                    mut reader,
                },
                _,
            ) => match reader.readable(&mut sock)? {
                ReadRes::Done(data) => {
                    // Some trackers incorrectly include trailing characters in the response.
                    let content = bencode::decode_buf_first(&data).map_err(|e| {
                        let data = std::str::from_utf8(&data)
                            .map_or_else(|_| format!("{data:?}"), str::to_string);
                        Error::ResponseInvalidBencode(data, e)
                    })?;
                    let resp = TrackerResponse::from_bencode(content)?;
                    Ok(TrackerState::Complete(resp))
                }
                ReadRes::Redirect(l) => Ok(TrackerState::Redirect(l)),
                ReadRes::None => Ok(TrackerState::Reading { sock, reader }),
            },
            (s @ TrackerState::ResolvingDNS { .. }, _) => Ok(s),
            _ => Err(Error::BadStateTransition),
        }
    }
}

impl Handler {
    pub fn new(reg: &amy::Registrar) -> io::Result<Handler> {
        Ok(Handler {
            reg: reg.clone(),
            connections: UHashMap::default(),
        })
    }

    pub fn active_requests(&self) -> usize {
        self.connections.len()
    }

    pub fn complete(&self) -> bool {
        self.connections.is_empty()
    }

    pub fn contains(&self, id: usize) -> bool {
        self.connections.contains_key(&id)
    }

    pub fn dns_resolved(&mut self, resp: dns::QueryResponse) -> Option<Response> {
        let id = resp.id;
        debug!("Received a DNS resp for {:?}", id);
        let resp = if let Some(trk) = self.connections.get_mut(&id) {
            trk.last_updated = Instant::now();
            match trk.state.handle(Event::DNSResolved(resp)) {
                Ok(_) => None,
                Err(e) => Some(Response::Tracker {
                    tid: trk.torrent,
                    url: trk.url.clone(),
                    resp: Err(e),
                }),
            }
        } else {
            None
        };
        if resp.is_some() {
            self.connections.remove(&id);
        }
        resp
    }

    pub fn writable(&mut self, id: usize) -> Option<Response> {
        let resp = if let Some(trk) = self.connections.get_mut(&id) {
            trk.last_updated = Instant::now();
            match trk.state.handle(Event::Writable) {
                Ok(_) => None,
                Err(e) => Some(Response::Tracker {
                    tid: trk.torrent,
                    url: trk.url.clone(),
                    resp: Err(e),
                }),
            }
        } else {
            None
        };
        if resp.is_some() {
            self.connections.remove(&id);
        }
        resp
    }

    pub fn readable(&mut self, id: usize, dns: &mut dns::Resolver) -> Option<Response> {
        let mut loc = None;
        let mut resp = if let Some(trk) = self.connections.get_mut(&id) {
            trk.last_updated = Instant::now();
            match trk.state.handle(Event::Readable) {
                Ok(HTTPRes::Complete(r)) => {
                    debug!("Announce response received for {:?} succesfully", id);
                    Some(Response::Tracker {
                        tid: trk.torrent,
                        url: trk.url.clone(),
                        resp: Ok(r),
                    })
                }
                Ok(HTTPRes::Redirect(l)) => {
                    loc = Some((l, trk.url.clone()));
                    None
                }
                Ok(HTTPRes::None) => None,
                Err(e) => Some(Response::Tracker {
                    tid: trk.torrent,
                    url: trk.url.clone(),
                    resp: Err(e),
                }),
            }
        } else {
            None
        };

        if resp.is_some() {
            self.connections.remove(&id);
        }

        if let Some((l, old)) = loc {
            let trk = self.connections.remove(&id).unwrap();
            // Disallow 2 levels of redirection
            if trk.redirect {
                resp = Some(Response::Tracker {
                    tid: trk.torrent,
                    url: trk.url.clone(),
                    resp: Err(Error::TooManyRedirects),
                });
            }
            if let Err(e) = self.try_redirect(&l, old, trk.torrent, dns) {
                debug!(
                    "Announce response received for {:?}, redirecting!",
                    trk.torrent
                );
                resp = Some(Response::Tracker {
                    tid: trk.torrent,
                    url: trk.url,
                    resp: Err(e),
                });
            }
        }
        resp
    }

    fn try_redirect(
        &mut self,
        url: &str,
        original_url: Arc<Url>,
        torrent: usize,
        dns: &mut dns::Resolver,
    ) -> Result<()> {
        let url = match Url::parse(url) {
            Ok(url) => Ok(url),
            Err(url::ParseError::RelativeUrlWithoutBase) => original_url.join(url),
            Err(e) => Err(e),
        }
        .map_err(|e| Error::UrlParse("malformed redirect", url.to_string(), e))?;
        let Some(host) = url.host_str() else {
            return Err(Error::UrlNoHost(url.into()));
        };
        let mut http_req = Vec::with_capacity(512);
        http::RequestBuilder::new("GET", url.path(), url.query())
            .header("User-agent", concat!("synapse/", env!("CARGO_PKG_VERSION")))
            .header("Connection", "close")
            .header("Host", host)
            .encode(&mut http_req);

        let ohost = if url.scheme() == "https" {
            Some(host.to_owned())
        } else {
            None
        };

        // Setup actual connection and start DNS query
        let sock = SStream::new_v4(ohost).map_err(Error::CreateSocket)?;
        let id = self
            .reg
            .register(&sock, amy::Event::Both)
            .map_err(Error::Registrar)?;
        let port = url.port().unwrap_or(80);
        self.connections.insert(
            id,
            Tracker {
                last_updated: Instant::now(),
                redirect: true,
                torrent,
                url: original_url,
                state: TrackerState::new(sock, http_req, port),
            },
        );

        debug!("Dispatching redirect DNS req, id {:?}", id);
        if let Some(ip) = dns.new_query(id, host).map_err(Error::DnsIo)? {
            debug!("Using cached DNS response");
            let res = self.dns_resolved(dns::QueryResponse { id, res: Ok(ip) });
            if res.is_some() {
                return Err(Error::Connection);
            }
        }
        // TODO: Should the None branch be an error?
        Ok(())
    }

    pub fn tick(&mut self) -> Vec<Response> {
        let mut resps = Vec::new();
        self.connections.retain(|id, trk| {
            if trk.last_updated.elapsed() > Duration::from_millis(TIMEOUT_MS) {
                debug!("Announce {:?} timed out", id);
                resps.push(Response::Tracker {
                    tid: trk.torrent,
                    url: trk.url.clone(),
                    resp: Err(Error::Timeout),
                });
                false
            } else {
                true
            }
        });
        resps
    }

    pub fn new_announce(&mut self, req: Announce, dns: &mut dns::Resolver) -> Result<()> {
        debug!("Received a new announce req for {:?}", req.url);
        let host = req
            .url
            .host_str()
            .ok_or_else(|| Error::UrlNoHost(req.url.as_ref().clone().into()))?;

        let mut http_req = Vec::with_capacity(512);
        let num_want = req.num_want.map(|nw| nw.to_string());
        let event = match req.event {
            Some(tracker::Event::Started) => Some("started"),
            Some(tracker::Event::Stopped) => Some("stopped"),
            Some(tracker::Event::Completed) => Some("completed"),
            None => None,
        };
        http::RequestBuilder::new("GET", req.url.path(), req.url.query())
            .query("info_hash", &req.hash)
            .query("peer_id", &PEER_ID[..])
            .query("uploaded", req.uploaded.to_string().as_bytes())
            .query("downloaded", req.downloaded.to_string().as_bytes())
            .query("left", req.left.to_string().as_bytes())
            .query("compact", b"1")
            .query("port", req.port.to_string().as_bytes())
            .query_opt("numwant", num_want.as_ref().map(|nw| nw.as_bytes()))
            .query_opt("event", event.map(|e| e.as_bytes()))
            .header("User-agent", concat!("synapse/", env!("CARGO_PKG_VERSION")))
            .header("Connection", "close")
            .header("Host", host)
            .encode(&mut http_req);

        let port = req
            .url
            .port()
            .unwrap_or_else(|| if req.url.scheme() == "https" { 443 } else { 80 });

        let ohost = if req.url.scheme() == "https" {
            Some(host.to_owned())
        } else {
            None
        };

        // Setup actual connection and start DNS query
        let sock = SStream::new_v4(ohost).map_err(Error::CreateSocket)?;
        let id = self
            .reg
            .register(&sock, amy::Event::Both)
            .map_err(Error::Registrar)?;
        self.connections.insert(
            id,
            Tracker {
                url: req.url.clone(),
                last_updated: Instant::now(),
                torrent: req.id,
                state: TrackerState::new(sock, http_req, port),
                redirect: false,
            },
        );

        debug!("Dispatching DNS req, id {:?}", id);
        if let Some(ip) = dns.new_query(id, host).map_err(Error::DnsIo)? {
            debug!("Using cached DNS response");
            let res = self.dns_resolved(dns::QueryResponse { id, res: Ok(ip) });
            if res.is_some() {
                return Err(Error::Connection);
            }
        }

        Ok(())
    }
}
