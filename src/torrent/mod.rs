pub mod info;
pub mod peer;
pub mod bitfield;
mod picker;
mod choker;

use chrono::{DateTime, Utc};

pub use self::bitfield::Bitfield;
pub use self::info::Info;
pub use self::peer::{Peer, PeerConn};

pub use self::peer::Message;
use self::picker::Picker;
use std::fmt;
use control::cio;
use {bincode, rpc, disk, RAREST_PKR};
use rpc::resource::{self, Resource, SResourceUpdate};
use throttle::Throttle;
use tracker::{self, TrackerResponse};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use util;
use slog::Logger;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum TrackerStatus {
    Updating,
    Ok {
        seeders: u32,
        leechers: u32,
        interval: u32,
    },
    Failure(String),
    Error,
}

#[derive(Serialize, Deserialize)]
struct TorrentData {
    info: Info,
    pieces: Bitfield,
    uploaded: u64,
    downloaded: u64,
    picker: Picker,
    status: Status,
}

pub struct Torrent<T: cio::CIO> {
    id: usize,
    pieces: Bitfield,
    info: Arc<Info>,
    cio: T,
    uploaded: u64,
    downloaded: u64,
    last_ul: u32,
    last_dl: u32,
    last_clear: DateTime<Utc>,
    throttle: Throttle,
    tracker: TrackerStatus,
    tracker_update: Option<Instant>,
    peers: HashMap<usize, Peer<T>>,
    leechers: HashSet<usize>,
    picker: Picker,
    status: Status,
    choker: choker::Choker,
    l: Logger,
    dirty: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Status {
    Pending,
    Paused,
    Leeching,
    Idle,
    Seeding,
    Validating,
    DiskError,
}

impl Status {
    pub fn leeching(&self) -> bool {
        match *self {
            Status::Leeching => true,
            _ => false,
        }
    }

    pub fn stopped(&self) -> bool {
        match *self {
            Status::Paused | Status::DiskError => true,
            _ => false,
        }
    }
}

impl<T: cio::CIO> Torrent<T> {
    pub fn new(id: usize, info: Info, throttle: Throttle, cio: T, l: Logger) -> Torrent<T> {
        debug!(l, "Creating {:?}", info);
        // Create empty initial files
        let created = info.create_files().ok();
        let peers = HashMap::new();
        let pieces = Bitfield::new(info.pieces() as u64);
        let picker = if RAREST_PKR {
            Picker::new_rarest(&info)
        } else {
            Picker::new_sequential(&info)
        };
        let leechers = HashSet::new();
        let status = if created.is_some() {
            Status::Pending
        } else {
            Status::DiskError
        };
        let mut t = Torrent {
            id,
            info: Arc::new(info),
            peers,
            pieces,
            picker,
            uploaded: 0,
            downloaded: 0,
            last_ul: 0,
            last_dl: 0,
            last_clear: Utc::now(),
            cio,
            leechers,
            throttle,
            tracker: TrackerStatus::Updating,
            tracker_update: None,
            choker: choker::Choker::new(),
            l: l.clone(),
            dirty: false,
            status,
        };
        t.start();

        t
    }

    pub fn deserialize(
        id: usize,
        data: &[u8],
        throttle: Throttle,
        cio: T,
        l: Logger,
        ) -> Result<Torrent<T>, bincode::Error> {
        let mut d: TorrentData = bincode::deserialize(data)?;
        debug!(l, "Torrent data deserialized!");
        d.picker.unset_waiting();
        let peers = HashMap::new();
        let leechers = HashSet::new();
        let mut t = Torrent {
            id,
            info: Arc::new(d.info),
            peers,
            pieces: d.pieces,
            picker: d.picker,
            uploaded: d.uploaded,
            downloaded: d.downloaded,
            last_ul: 0,
            last_dl: 0,
            last_clear: Utc::now(),
            cio,
            leechers,
            throttle,
            tracker: TrackerStatus::Updating,
            tracker_update: None,
            choker: choker::Choker::new(),
            l: l.clone(),
            dirty: false,
            status: d.status,
        };
        match t.status {
            Status::DiskError | Status::Seeding | Status::Leeching => {
                if t.pieces.complete() {
                    t.status = Status::Idle;
                } else {
                    t.status = Status::Pending;
                }
            }
            Status::Validating => {
                t.validate();
            }
            _ => {}
        };
        t.start();
        Ok(t)
    }

    pub fn serialize(&mut self) {
        let d = TorrentData {
            info: self.info.as_ref().clone(),
            pieces: self.pieces.clone(),
            uploaded: self.uploaded,
            downloaded: self.downloaded,
            picker: self.picker.clone(),
            status: self.status,
        };
        let data = bincode::serialize(&d, bincode::Infinite).expect("Serialization failed!");
        debug!(self.l, "Sending serialization request!");
        self.cio.msg_disk(disk::Request::serialize(
                self.id,
                data,
                self.info.hash,
                ));
        self.dirty = false;
    }

    pub fn rpc_id(&self) -> String {
        util::hash_to_id(&self.info.hash[..])
    }

    pub fn delete(&mut self) {
        debug!(self.l, "Sending file deletion request!");
        self.cio.msg_disk(
            disk::Request::delete(self.id, self.info.hash),
            );
    }

    pub fn set_tracker_response(&mut self, resp: &tracker::Result<TrackerResponse>) {
        debug!(self.l, "Processing tracker response");
        match *resp {
            Ok(ref r) => {
                let mut time = Instant::now();
                time += Duration::from_secs(r.interval as u64);
                self.tracker = TrackerStatus::Ok {
                    seeders: r.seeders,
                    leechers: r.leechers,
                    interval: r.interval,
                };
                self.tracker_update = Some(time);
            }
            Err(tracker::Error(tracker::ErrorKind::TrackerError(ref s), _)) => {
                self.tracker = TrackerStatus::Failure(s.clone());
            }
            Err(ref e) => {
                warn!(self.l, "Failed to query tracker: {:?}", e.backtrace());
                self.tracker = TrackerStatus::Error;
            }
        }
    }

    pub fn update_tracker(&mut self) {
        if let Some(end) = self.tracker_update {
            debug!(self.l, "Updating tracker at inteval!");
            let cur = Instant::now();
            if cur >= end {
                let req = tracker::Request::interval(self);
                self.cio.msg_trk(req);
            }
        }
    }

    pub fn get_throttle(&self, id: usize) -> Throttle {
        self.throttle.new_sibling(id)
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn uploaded(&self) -> u64 {
        self.uploaded
    }

    pub fn downloaded(&self) -> u64 {
        self.downloaded
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    pub fn handle_disk_resp(&mut self, resp: disk::Response) {
        match resp {
            disk::Response::Read { context, data } => {
                trace!(self.l, "Received piece from disk, uploading!");
                if let Some(peer) = self.peers.get_mut(&context.pid) {
                    let p = Message::s_piece(context.idx, context.begin, context.length, data);
                    // This may not be 100% accurate, but close enough for now.
                    self.uploaded += context.length as u64;
                    self.last_ul += context.length as u32;
                    self.dirty = true;
                    peer.send_message(p);
                }
            }
            disk::Response::ValidationComplete { invalid, .. } => {
                debug!(self.l, "Validation completed!");
                if invalid.is_empty() {
                    info!(self.l, "Torrent succesfully downloaded!");
                    // TOOD: Consider if we should cache this
                    if !self.status.stopped() {
                        self.set_status(Status::Idle);
                    }
                    let req = tracker::Request::completed(self);
                    self.cio.msg_trk(req);
                } else {
                    warn!(
                        self.l,
                        "Torrent has incorrect pieces {:?}, redownloading",
                        invalid
                        );
                    for piece in invalid {
                        self.picker.invalidate_piece(piece);
                    }
                    self.request_all();
                }
            }
            disk::Response::Error { err, .. } => {
                warn!(self.l, "Disk error: {:?}", err);
                self.set_status(Status::DiskError);
            }
        }
    }

    pub fn peer_ev(&mut self, pid: cio::PID, evt: cio::Result<Message>) -> Result<(), ()> {
        let mut peer = self.peers.remove(&pid).ok_or(())?;
        if let Ok(mut msg) = evt {
            if peer.handle_msg(&mut msg).is_ok() && self.handle_msg(msg, &mut peer).is_ok() {
                self.peers.insert(pid, peer);
                return Ok(());
            }
        }
        self.cleanup_peer(&mut peer);
        Err(())
    }

    pub fn handle_msg(&mut self, msg: Message, peer: &mut Peer<T>) -> Result<(), ()> {
        trace!(self.l, "Received {:?} from peer", msg);
        match msg {
            Message::Handshake { .. } => {
                debug!(self.l, "Connection established with peer {:?}", peer.id());
            }
            Message::Bitfield(_) => {
                if self.pieces.usable(peer.pieces()) {
                    self.picker.add_peer(peer);
                    peer.interested();
                }
                if !peer.pieces().complete() {
                    self.leechers.insert(peer.id());
                }
            }
            Message::Have(idx) => {
                if peer.pieces().complete() {
                    self.leechers.remove(&peer.id());
                }
                if self.pieces.usable(peer.pieces()) {
                    peer.interested();
                }
                self.picker.piece_available(idx);
            }
            Message::Unchoke => {
                debug!(self.l, "Unchoked by: {:?}!", peer);
                self.make_requests(peer);
            }
            Message::Piece {
                index,
                begin,
                data,
                length,
            } => {
                // Ignore a piece we already have, this could happen from endgame
                if self.pieces.has_bit(index as u64) {
                    return Ok(());
                }

                // Even though we have the data, if we are stopped we shouldn't use the disk
                // regardless.
                if !self.status.stopped() {
                    self.set_status(Status::Leeching);
                } else {
                    return Ok(());
                }

                if self.info.block_len(index, begin) != length {
                    return Err(());
                }

                // Internal data structures which are being serialized have changed, flag self as
                // dirty
                self.dirty = true;
                self.write_piece(index, begin, data);

                self.downloaded += length as u64;
                self.last_dl += length as u32;
                let (piece_done, mut peers) = self.picker.completed(index, begin);
                if piece_done {
                    self.pieces.set_bit(index as u64);

                    // Begin validation, and save state if the torrent is done
                    if self.pieces.complete() {

                        debug!(self.l, "Beginning validation");
                        self.serialize();
                        self.validate();
                    }

                    // Tell all relevant peers we got the piece
                    let m = Message::Have(index);
                    for pid in self.leechers.iter() {
                        if let Some(peer) = self.peers.get_mut(pid) {
                            if !peer.pieces().has_bit(index as u64) {
                                peer.send_message(m.clone());
                            }
                        } else {
                            warn!(self.l, "PID {} in leechers not found in peers.", pid);
                        }
                    }

                    // Mark uninteresting peers
                    for (_, peer) in self.peers.iter_mut() {
                        if !self.pieces.usable(peer.pieces()) {
                            peer.uninterested();
                        }
                    }
                }

                // If there are any peers we've asked duplicate pieces for(due to endgame),
                // cancel it, though we should still assume they'll probably send it anyways
                if peers.len() > 1 {
                    peers.remove(&peer.id());
                    let m = Message::Cancel {
                        index,
                        begin,
                        length,
                    };
                    for pid in peers {
                        if let Some(peer) = self.peers.get_mut(&pid) {
                            peer.send_message(m.clone());
                        }
                    }
                }

                if !self.pieces.complete() {
                    self.make_requests(peer);
                }
            }
            Message::Request {
                index,
                begin,
                length,
            } => {
                if !self.status.stopped() && !self.status.leeching() {
                    self.set_status(Status::Seeding);
                    // TODO get this from some sort of allocator.
                    if length != self.info.block_len(index, begin) {
                        return Err(());
                    } else {
                        self.request_read(peer.id(), index, begin, Box::new([0u8; 16384]));
                    }
                } else {
                    // TODO: add this to a queue to fulfill later
                }
            }
            Message::Interested => {
                self.choker.add_peer(peer);
            }
            Message::Uninterested => {
                self.choker.remove_peer(peer, &mut self.peers);
            }
            Message::KeepAlive |
                Message::Choke |
                Message::Cancel { .. } |
                Message::Port(_) => {}

            Message::SharedPiece { .. } => unreachable!(),
        }
        Ok(())
    }

    /// Periodically called to update peers, choking the slowest one and
    /// optimistically unchoking a new peer
    pub fn update_unchoked(&mut self) {
        if self.complete() {
            self.choker.update_download(&mut self.peers)
        } else {
            self.choker.update_upload(&mut self.peers)
        };
    }

    pub fn rpc_update(&mut self, u: rpc::proto::resource::CResourceUpdate) {
        if let Some(status) = u.status {
            match (status, self.status) {
                (resource::Status::Paused, Status::Paused) => {
                    self.resume();
                }
                (resource::Status::Paused, _) => {
                    self.pause();
                }
                (resource::Status::Hashing, Status::Validating) => { }
                (resource::Status::Hashing, _) => {
                    self.validate();
                }
                // The rpc module should handle invalid status requests.
                _ => { }
            }
        }

        if u.throttle_up.is_some() || u.throttle_down.is_some() {
            let tu = u.throttle_up.unwrap_or(self.throttle.ul_rate() as u32);
            let td = u.throttle_down.unwrap_or(self.throttle.dl_rate() as u32);
            self.set_throttle(tu, td);
        }

        if let Some(p) = u.path {
            // TODO: Implement custom paths
        }

        if let Some(p) = u.priority {
            // TODO: Implement priority
        }

        if let Some(s) = u.sequential {
            if s {
                let p = Picker::new_sequential(&self.info);
                self.change_picker(p);
            } else {
                let p = Picker::new_rarest(&self.info);
                self.change_picker(p);
            }
        }
    }

    fn start(&mut self) {
        debug!(self.l, "Sending start request");
        let req = tracker::Request::started(self);
        self.cio.msg_trk(req);
        // TODO: Consider repeatedly sending out these during annoucne intervals
        if !self.info.private {
            let mut req = tracker::Request::DHTAnnounce(self.info.hash);
            self.cio.msg_trk(req);
            req = tracker::Request::GetPeers(tracker::GetPeers {
                id: self.id,
                hash: self.info.hash,
            });
            self.cio.msg_trk(req);
        }

        // Update RPC of the torrent, tracker, files, and peers
        let resources = self.rpc_info();
        self.cio.msg_rpc(rpc::CtlMessage::Extant(resources));
    }

    fn complete(&self) -> bool {
        self.pieces.complete()
    }

    fn set_throttle(&mut self, ul: u32, dl: u32) {
        self.throttle.set_ul_rate(ul as usize);
        self.throttle.set_dl_rate(dl as usize);
        let id = self.rpc_id();
        self.cio.msg_rpc(rpc::CtlMessage::Update(vec![resource::SResourceUpdate::Throttle {
            id,
            throttle_up: ul,
            throttle_down: dl,
        }]));
    }

    fn rpc_info(&self) -> Vec<resource::Resource> {
        let mut r = Vec::new();
        r.push(Resource::Torrent(resource::Torrent {
            id: self.rpc_id(),
            name: self.info.name.clone(),
            // TODO: Properly add this
            path: "./".to_owned(),
            created: Utc::now(),
            modified: Utc::now(),
            status: self.status.into(),
            error: self.error(),
            priority: 3,
            progress: self.progress(),
            availability: self.availability(),
            sequential: self.sequential(),
            rate_up: 0,
            rate_down: 0,
            // TODO: COnsider the overflow potential here
            throttle_up: self.throttle.ul_rate() as u32,
            throttle_down: self.throttle.dl_rate() as u32,
            transferred_up: self.uploaded,
            transferred_down: self.downloaded,
            peers: 0,
            // TODO: Alter when mutlitracker support hits
            trackers: 1,
            pieces: self.info.pieces() as u64,
            piece_size: self.info.piece_len,
            files: self.info.files.len() as u32,
        }));

        for i in 0..self.info.pieces() {
            let id = util::piece_rpc_id(&self.info.hash, i as u64);
            // TODO: Formalize these high bit ids
            if self.pieces.has_bit(i as u64) {
                r.push(Resource::Piece(resource::Piece {
                    // TODO: Formalize these high bit ids
                    id,
                    torrent_id: self.rpc_id(),
                    available: true,
                    downloaded: true,
                }))
            } else {
                r.push(Resource::Piece(resource::Piece {
                    id,
                    torrent_id: self.rpc_id(),
                    available: true,
                    downloaded: false,
                }))
            }
        }

        for (i, f) in self.info.files.iter().enumerate() {
            let id = util::file_rpc_id(&self.info.hash, f.path.as_path().to_string_lossy().as_ref());
            r.push(resource::Resource::File(resource::File {
                id,
                torrent_id: self.rpc_id(),
                availability: 0.,
                progress: 0.,
                priority: 3,
                path: f.path.as_path().to_string_lossy().into_owned(),
            }))
        }

        // TODO: Send trackers too

        r
    }

    fn error(&self) -> Option<String> {
        match self.status {
            Status::DiskError => Some("Disk error!".to_owned()),
            _ => None,
        }
    }

    fn sequential(&self) -> bool {
        match &self.picker {
            &Picker::Sequential(_) => true,
            _ => false,
        }
    }

    fn progress(&self) -> f32 {
        self.pieces.iter().count() as f32 / self.info.pieces() as f32
    }

    fn availability(&self) -> f32 {
        // TODO: ??
        0.
    }

    pub fn reset_last_tx_rate(&mut self) -> (u32, u32) {
        let res = self.get_last_tx_rate();
        self.last_clear = Utc::now();
        self.last_ul = 0;
        self.last_dl = 0;
        res
    }

    // TODO: Implement Exp Moving Avg Somewhere
    pub fn get_last_tx_rate(&self) -> (u32, u32) {
        let dur = Utc::now()
            .signed_duration_since(self.last_clear)
            .num_milliseconds() as u32;
        let ul = 1000 * (self.last_ul / dur);
        let dl = 1000 * (self.last_dl / dur);
        (ul, dl)
    }

    /// Writes a piece of torrent info, with piece index idx,
    /// piece offset begin, piece length of len, and data bytes.
    /// The disk send handle is also provided.
    fn write_piece(&mut self, index: u32, begin: u32, data: Box<[u8; 16384]>) {
        let locs = self.info.block_disk_locs(index, begin);
        self.cio.msg_disk(disk::Request::write(self.id, data, locs));
    }

    /// Issues a read request of the given torrent
    fn request_read(&mut self, id: usize, index: u32, begin: u32, data: Box<[u8; 16384]>) {
        let locs = self.info.block_disk_locs(index, begin);
        let len = self.info.block_len(index, begin);
        let ctx = disk::Ctx::new(id, self.id, index, begin, len);
        self.cio.msg_disk(disk::Request::read(ctx, data, locs));
    }

    fn make_requests_pid(&mut self, pid: usize) {
        let peer = self.peers.get_mut(&pid).expect(
            "Expected peer id not present",
            );
        if self.status.stopped() {
            return;
        }
        while peer.can_queue_req() {
            if let Some((idx, offset)) = self.picker.pick(peer) {
                peer.request_piece(idx, offset, self.info.block_len(idx, offset));
            } else {
                break;
            }
        }
    }

    fn make_requests(&mut self, peer: &mut Peer<T>) {
        if self.status.stopped() {
            return;
        }
        while peer.can_queue_req() {
            if let Some((idx, offset)) = self.picker.pick(peer) {
                peer.request_piece(idx, offset, self.info.block_len(idx, offset));
            } else {
                break;
            }
        }
    }

    pub fn add_peer(&mut self, conn: PeerConn) -> Option<usize> {
        if let Ok(p) = Peer::new(conn, self) {
            let pid = p.id();
            debug!(self.l, "Adding peer {:?}!", pid);
            self.picker.add_peer(&p);
            self.peers.insert(pid, p);
            Some(pid)
        } else {
            None
        }
    }

    fn set_status(&mut self, status: Status) {
        if self.status == status {
            return;
        }
        self.status = status;
        let id = self.rpc_id();
        self.cio.msg_rpc(rpc::CtlMessage::Update(vec![
                                                 SResourceUpdate::TorrentStatus {
                                                     id,
                                                     error: match status {
                                                         Status::DiskError => Some("Disk error".to_owned()),
                                                         _ => None,
                                                     },
                                                     status: status.into(),
                                                 },
        ]));
    }

    pub fn update_rpc_peers(&mut self) {
        let availability = self.availability();
        let id = self.rpc_id();
        self.cio.msg_rpc(rpc::CtlMessage::Update(vec![
                                                 SResourceUpdate::TorrentPeers {
                                                     id,
                                                     peers: self.peers.len() as u16,
                                                     availability,
                                                 },
        ]));
    }

    pub fn update_rpc_transfer(&mut self) {
        let availability = self.availability();
        let progress = self.progress();
        let (rate_up, rate_down) = self.get_last_tx_rate();
        let id = self.rpc_id();
        self.cio.msg_rpc(rpc::CtlMessage::Update(vec![
                                                 SResourceUpdate::TorrentTransfer {
                                                     id,
                                                     rate_up,
                                                     rate_down,
                                                     transferred_up: self.uploaded,
                                                     transferred_down: self.downloaded,
                                                     progress,
                                                 },
        ]));
    }

    fn cleanup_peer(&mut self, peer: &mut Peer<T>) {
        debug!(self.l, "Removing peer {:?}!", peer);
        self.choker.remove_peer(peer, &mut self.peers);
        self.leechers.remove(&peer.id());
        self.picker.remove_peer(&peer);
    }

    pub fn pause(&mut self) {
        debug!(self.l, "Pausing torrent!");
        match self.status {
            Status::Paused => {}
            _ => {
                debug!(self.l, "Sending stopped request to trk");
                let req = tracker::Request::stopped(self);
                self.cio.msg_trk(req);
            }
        }
        self.set_status(Status::Paused);
    }

    pub fn resume(&mut self) {
        debug!(self.l, "Resuming torrent!");
        match self.status {
            Status::Paused => {
                debug!(self.l, "Sending started request to trk");
                let req = tracker::Request::started(self);
                self.cio.msg_trk(req);
                self.request_all();
            }
            Status::DiskError => {
                if self.pieces.complete() {
                    self.validate();
                } else {
                    self.request_all();
                    self.set_status(Status::Idle);
                }
            }
            _ => {}
        }
        if self.pieces.complete() {
            self.set_status(Status::Idle);
        } else {
            self.set_status(Status::Pending);
        }
    }

    fn validate(&mut self) {
        self.cio.msg_disk(
            disk::Request::validate(self.id, self.info.clone()),
            );
        self.set_status(Status::Validating);
    }

    fn request_all(&mut self) {
        for pid in self.pids() {
            self.make_requests_pid(pid);
        }
    }

    fn pids(&self) -> Vec<usize> {
        self.peers.keys().cloned().collect()
    }

    // TODO: use this over RPC
    #[allow(dead_code)]
    pub fn change_picker(&mut self, mut picker: Picker) {
        debug!(self.l, "Swapping pickers!");
        for (_, peer) in self.peers.iter() {
            picker.add_peer(peer);
        }
        self.picker.change_picker(picker);
        let id = self.rpc_id();
        let sequential = self.picker.is_sequential();
        self.cio.msg_rpc(rpc::CtlMessage::Update(vec![
                                                 SResourceUpdate::TorrentPicker {
                                                     id,
                                                     sequential,
                                                 }
        ]));
    }
}

impl<T: cio::CIO> fmt::Debug for Torrent<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Torrent {{ info: {:?} }}", self.info)
    }
}

impl<T: cio::CIO> fmt::Display for Torrent<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Torrent {}", util::hash_to_id(&self.info.hash[..]))
    }
}

impl<T: cio::CIO> Drop for Torrent<T> {
    fn drop(&mut self) {
        debug!(self.l, "Removing peers");
        for (id, peer) in self.peers.drain() {
            trace!(self.l, "Removing peer {:?}", peer);
            self.cio.remove_peer(id);
            self.leechers.remove(&id);
        }
        match self.status {
            Status::Paused => {}
            _ => {
                let req = tracker::Request::stopped(self);
                self.cio.msg_trk(req);
            }
        }
    }
}

impl Into<rpc::resource::Status> for Status {
    fn into(self) -> rpc::resource::Status {
        match self {
            Status::Pending => rpc::resource::Status::Pending,
            Status::Paused => rpc::resource::Status::Paused,
            Status::Idle => rpc::resource::Status::Idle,
            Status::Leeching => rpc::resource::Status::Leeching,
            Status::Seeding => rpc::resource::Status::Seeding,
            Status::Validating => rpc::resource::Status::Hashing,
            Status::DiskError => rpc::resource::Status::Error,
        }
    }
}
