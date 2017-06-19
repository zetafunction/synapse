use std::thread;
use std::io::ErrorKind;
use std::net::{SocketAddrV4, Ipv4Addr, TcpListener};
use amy::{self, Poller, Registrar};
use std::collections::HashMap;
use std::sync::mpsc;
use slog::Logger;
use torrent::Peer;
use {control, CONTROL, CONFIG, TC};

pub struct Listener {
    listener: TcpListener,
    lid: usize,
    incoming: HashMap<usize, Peer>,
    poll: Poller,
    reg: Registrar,
    rx: mpsc::Receiver<Request>,
    l: Logger,
}

pub struct Handle {
    pub tx: mpsc::Sender<Request>,
}

impl Handle {
    pub fn init(&self) { }
}

unsafe impl Sync for Handle {}

pub enum Request {
    Shutdown,
}

impl Listener {
    pub fn new(rx: mpsc::Receiver<Request>, l: Logger) -> Listener {
        let ip = Ipv4Addr::new(0, 0, 0, 0);
        let port = CONFIG.get().port;
        debug!(l, "Binding to port {:?}!", port);
        let listener = TcpListener::bind(SocketAddrV4::new(ip, port)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let poll = Poller::new().unwrap();
        let reg = poll.get_registrar().unwrap();
        let lid = reg.register(&listener, amy::Event::Both).unwrap();

        Listener {
            listener,
            lid,
            incoming: HashMap::new(),
            poll,
            reg,
            rx,
            l,
        }
    }

    pub fn run(&mut self) {
        debug!(self.l, "Accepting connections!");
        loop {
            let res = if let Ok(r) = self.poll.wait(15) { r } else { break; };
            for not in res {
                match not.id {
                    id if id == self.lid => self.handle_conn(),
                    _ => self.handle_peer(not),
                }
            }
            if let Ok(Request::Shutdown) = self.rx.try_recv() {
                break;
            }
        }
        debug!(self.l, "Shut down!");
    }

    fn handle_conn(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((conn, _ip)) => {
                    debug!(self.l, "Accepted new connection from {:?}!", _ip);
                    let peer = Peer::new_incoming(conn).unwrap();
                    let pid = self.reg.register(&peer.conn, amy::Event::Read).unwrap();
                    self.incoming.insert(pid, peer);
                }
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                    break;
                }
                _ => { unimplemented!(); }
            }
        }
    }

    fn handle_peer(&mut self, not: amy::Notification) {
        let pid = not.id;
        let res = self.incoming.get_mut(&pid).unwrap().read();
        match res {
            Ok(Some(hs)) => {
                debug!(self.l, "Completed handshake({:?}) with peer, transferring!", hs);
                let peer = self.incoming.remove(&pid).unwrap();
                self.reg.deregister(&peer.conn).unwrap();
                CONTROL.ctrl_tx.lock().unwrap().send(control::Request::AddPeer(peer, hs.get_handshake_hash())).unwrap();
            }
            Ok(_) => { }
            Err(e) => {
                debug!(self.l, "Peer connection failed: {:?}!", e);
                self.incoming.remove(&pid);
            }
        }
    }
}

pub fn start(l: Logger) -> Handle {
    debug!(l, "Initializing!");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        Listener::new(rx, l).run();
        use std::sync::atomic;
        TC.fetch_sub(1, atomic::Ordering::SeqCst);
    });
    Handle { tx }
}
