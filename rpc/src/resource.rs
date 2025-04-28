use std::borrow::Cow;
use std::fmt;
use std::mem;

use chrono::prelude::{DateTime, Utc};
use serde;
use serde_json as json;
use url::Url;

use super::criterion::{Field, Queryable, FNULL};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type")]
#[serde(rename_all = "lowercase")]
pub enum Resource {
    Server(Server),
    Torrent(Torrent),
    Piece(Piece),
    File(File),
    Peer(Peer),
    Tracker(Tracker),
}

#[derive(Copy, Clone, Default, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "lowercase")]
pub enum ResourceKind {
    Server = 0,
    #[default]
    Torrent,
    Peer,
    File,
    Piece,
    Tracker,
}

/// To increase server->client update efficiency, we
/// encode common partial updates to resources with
/// this enum.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
pub enum SResourceUpdate<'a> {
    Resource(Cow<'a, Resource>),
    Throttle {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        throttle_up: Option<i64>,
        throttle_down: Option<i64>,
    },
    Rate {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        rate_up: u64,
        rate_down: u64,
    },
    UserData {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        user_data: json::Value,
    },

    ServerTransfer {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        rate_up: u64,
        rate_down: u64,
        transferred_up: u64,
        transferred_down: u64,
        ses_transferred_up: u64,
        ses_transferred_down: u64,
    },
    ServerSpace {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        free_space: u64,
    },
    ServerToken {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        download_token: String,
    },

    TorrentStatus {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        error: Option<String>,
        status: Status,
    },
    TorrentTransfer {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        rate_up: u64,
        rate_down: u64,
        transferred_up: u64,
        transferred_down: u64,
        progress: f32,
    },
    TorrentPeers {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        peers: u16,
        availability: f32,
    },
    TorrentPicker {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        strategy: Strategy,
    },
    TorrentPriority {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        priority: u8,
    },
    TorrentPath {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        path: String,
    },
    TorrentPieces {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        piece_field: String,
    },

    TrackerStatus {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        last_report: DateTime<Utc>,
        error: Option<String>,
    },

    FilePriority {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        priority: u8,
    },
    FileProgress {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        progress: f32,
    },

    PieceAvailable {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        available: bool,
    },
    PieceDownloaded {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        downloaded: bool,
    },

    PeerAvailability {
        id: String,
        #[serde(rename = "type")]
        kind: ResourceKind,
        availability: f32,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum PathUpdate {
    Move(String),
    MoveSkipFiles(String),
}

/// Collection of mutable fields that clients
/// can modify. Due to shared field names, all fields are aggregated
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CResourceUpdate {
    pub id: String,
    pub path: Option<PathUpdate>,
    pub priority: Option<u8>,
    pub strategy: Option<Strategy>,
    #[serde(deserialize_with = "deserialize_throttle")]
    #[serde(default)]
    pub throttle_up: Option<Option<i64>>,
    #[serde(deserialize_with = "deserialize_throttle")]
    #[serde(default)]
    pub throttle_down: Option<Option<i64>>,
    pub user_data: Option<json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub id: String,
    pub download_token: String,
    pub rate_up: u64,
    pub rate_down: u64,
    pub throttle_up: Option<i64>,
    pub throttle_down: Option<i64>,
    pub transferred_up: u64,
    pub transferred_down: u64,
    pub ses_transferred_up: u64,
    pub ses_transferred_down: u64,
    pub free_space: u64,
    pub started: DateTime<Utc>,
    pub user_data: json::Value,
}

impl Server {
    pub fn update(&mut self, update: SResourceUpdate<'_>) {
        match update {
            SResourceUpdate::Throttle {
                throttle_up,
                throttle_down,
                ..
            } => {
                self.throttle_up = throttle_up;
                self.throttle_down = throttle_down;
            }
            SResourceUpdate::ServerTransfer {
                rate_up,
                rate_down,
                transferred_up,
                transferred_down,
                ses_transferred_up,
                ses_transferred_down,
                ..
            } => {
                self.rate_up = rate_up;
                self.rate_down = rate_down;
                self.transferred_up = transferred_up;
                self.transferred_down = transferred_down;
                self.ses_transferred_up = ses_transferred_up;
                self.ses_transferred_down = ses_transferred_down;
            }
            SResourceUpdate::ServerToken { download_token, .. } => {
                self.download_token = download_token;
            }
            SResourceUpdate::ServerSpace { free_space, .. } => {
                self.free_space = free_space;
            }
            SResourceUpdate::Rate {
                rate_up, rate_down, ..
            } => {
                self.rate_up = rate_up;
                self.rate_down = rate_down;
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Torrent {
    pub id: String,
    pub name: Option<String>,
    pub creator: Option<String>,
    pub comment: Option<String>,
    pub private: bool,
    pub path: String,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    pub status: Status,
    pub error: Option<String>,
    pub priority: u8,
    pub progress: f32,
    pub availability: f32,
    pub strategy: Strategy,
    pub rate_up: u64,
    pub rate_down: u64,
    pub throttle_up: Option<i64>,
    pub throttle_down: Option<i64>,
    pub transferred_up: u64,
    pub transferred_down: u64,
    pub peers: u16,
    pub trackers: u8,
    pub tracker_urls: Vec<String>,
    pub size: Option<u64>,
    pub pieces: Option<u64>,
    pub piece_size: Option<u32>,
    pub piece_field: String,
    pub files: Option<u32>,
    pub user_data: json::Value,
}

impl Torrent {
    pub fn update(&mut self, update: SResourceUpdate<'_>) {
        self.modified = Utc::now();
        match update {
            SResourceUpdate::Throttle {
                throttle_up,
                throttle_down,
                ..
            } => {
                self.throttle_up = throttle_up;
                self.throttle_down = throttle_down;
            }
            SResourceUpdate::TorrentStatus { error, status, .. } => {
                self.error = error;
                self.status = status;
            }
            SResourceUpdate::TorrentTransfer {
                rate_up,
                rate_down,
                transferred_up,
                transferred_down,
                progress,
                ..
            } => {
                self.rate_up = rate_up;
                self.rate_down = rate_down;
                self.transferred_up = transferred_up;
                self.transferred_down = transferred_down;
                self.progress = progress;
            }
            SResourceUpdate::TorrentPath { path, .. } => {
                self.path = path;
            }
            SResourceUpdate::TorrentPeers {
                peers,
                availability,
                ..
            } => {
                self.peers = peers;
                self.availability = availability;
            }
            SResourceUpdate::TorrentPicker { strategy, .. } => {
                self.strategy = strategy;
            }
            SResourceUpdate::TorrentPriority { priority, .. } => {
                self.priority = priority;
            }
            SResourceUpdate::TorrentPieces { piece_field, .. } => {
                self.piece_field = piece_field;
            }
            SResourceUpdate::Resource(Cow::Borrowed(Resource::Torrent(t))) => *self = t.clone(),
            SResourceUpdate::Resource(Cow::Owned(Resource::Torrent(mut t))) => {
                mem::swap(self, &mut t)
            }
            SResourceUpdate::Resource(_) => {
                panic!("Torrent should not be updated with invalid resource type")
            }
            _ => {}
        }
    }
}

#[derive(Copy, Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
#[serde(deny_unknown_fields)]
pub enum Status {
    #[default]
    Pending,
    Magnet,
    Paused,
    Leeching,
    Idle,
    Seeding,
    Hashing,
    Error,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
#[serde(deny_unknown_fields)]
pub enum Strategy {
    Rarest,
    Sequential,
}

impl Strategy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Strategy::Rarest => "rarest",
            Strategy::Sequential => "sequential",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Piece {
    pub id: String,
    pub torrent_id: String,
    pub available: bool,
    pub downloaded: bool,
    pub index: u32,
    pub user_data: json::Value,
}

impl Piece {
    pub fn update(&mut self, update: SResourceUpdate<'_>) {
        match update {
            SResourceUpdate::PieceAvailable { available, .. } => {
                self.available = available;
            }
            SResourceUpdate::PieceDownloaded { downloaded, .. } => {
                self.downloaded = downloaded;
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub id: String,
    pub torrent_id: String,
    pub path: String,
    pub progress: f32,
    pub availability: f32,
    pub priority: u8,
    pub size: u64,
    pub user_data: json::Value,
}

impl File {
    pub fn update(&mut self, update: SResourceUpdate<'_>) {
        match update {
            SResourceUpdate::FilePriority { priority, .. } => {
                self.priority = priority;
            }
            SResourceUpdate::FileProgress { progress, .. } => {
                self.progress = progress;
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    pub id: String,
    pub torrent_id: String,
    pub client_id: String,
    pub ip: String,
    pub rate_up: u64,
    pub rate_down: u64,
    pub availability: f32,
    pub user_data: json::Value,
}

impl Peer {
    pub fn update(&mut self, update: SResourceUpdate<'_>) {
        match update {
            SResourceUpdate::Rate {
                rate_up, rate_down, ..
            } => {
                self.rate_up = rate_up;
                self.rate_down = rate_down;
            }
            SResourceUpdate::PeerAvailability { availability, .. } => {
                self.availability = availability;
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Tracker {
    pub id: String,
    pub torrent_id: String,
    pub url: Url,
    pub last_report: DateTime<Utc>,
    pub error: Option<String>,
    pub user_data: json::Value,
}

impl Tracker {
    pub fn update(&mut self, update: SResourceUpdate<'_>) {
        if let SResourceUpdate::TrackerStatus {
            last_report, error, ..
        } = update
        {
            self.last_report = last_report;
            self.error = error;
        }
    }
}

impl SResourceUpdate<'_> {
    pub fn id(&self) -> &str {
        match self {
            SResourceUpdate::Resource(r) => r.id(),
            SResourceUpdate::Throttle { id, .. }
            | SResourceUpdate::Rate { id, .. }
            | SResourceUpdate::UserData { id, .. }
            | SResourceUpdate::ServerTransfer { id, .. }
            | SResourceUpdate::ServerToken { id, .. }
            | SResourceUpdate::ServerSpace { id, .. }
            | SResourceUpdate::TorrentStatus { id, .. }
            | SResourceUpdate::TorrentTransfer { id, .. }
            | SResourceUpdate::TorrentPeers { id, .. }
            | SResourceUpdate::TorrentPicker { id, .. }
            | SResourceUpdate::TorrentPriority { id, .. }
            | SResourceUpdate::TorrentPath { id, .. }
            | SResourceUpdate::TorrentPieces { id, .. }
            | SResourceUpdate::FilePriority { id, .. }
            | SResourceUpdate::FileProgress { id, .. }
            | SResourceUpdate::TrackerStatus { id, .. }
            | SResourceUpdate::PeerAvailability { id, .. }
            | SResourceUpdate::PieceAvailable { id, .. }
            | SResourceUpdate::PieceDownloaded { id, .. } => id,
        }
    }
}

impl Resource {
    pub fn id(&self) -> &str {
        match self {
            Resource::Server(t) => &t.id,
            Resource::Torrent(t) => &t.id,
            Resource::File(t) => &t.id,
            Resource::Piece(t) => &t.id,
            Resource::Peer(t) => &t.id,
            Resource::Tracker(t) => &t.id,
        }
    }

    pub fn torrent_id(&self) -> Option<&str> {
        match self {
            Resource::File(t) => Some(&t.torrent_id),
            Resource::Piece(t) => Some(&t.torrent_id),
            Resource::Peer(t) => Some(&t.torrent_id),
            Resource::Tracker(t) => Some(&t.torrent_id),
            _ => None,
        }
    }

    pub fn kind(&self) -> ResourceKind {
        match self {
            Resource::Server(_) => ResourceKind::Server,
            Resource::Torrent(_) => ResourceKind::Torrent,
            Resource::File(_) => ResourceKind::File,
            Resource::Piece(_) => ResourceKind::Piece,
            Resource::Peer(_) => ResourceKind::Peer,
            Resource::Tracker(_) => ResourceKind::Tracker,
        }
    }

    pub fn user_data(&mut self) -> &mut json::Value {
        match self {
            Resource::Server(r) => &mut r.user_data,
            Resource::Torrent(r) => &mut r.user_data,
            Resource::File(r) => &mut r.user_data,
            Resource::Piece(r) => &mut r.user_data,
            Resource::Peer(r) => &mut r.user_data,
            Resource::Tracker(r) => &mut r.user_data,
        }
    }

    pub fn as_server(&self) -> &Server {
        match self {
            Resource::Server(s) => s,
            _ => panic!(),
        }
    }

    pub fn as_torrent(&self) -> &Torrent {
        match self {
            Resource::Torrent(t) => t,
            _ => panic!(),
        }
    }

    pub fn as_torrent_mut(&mut self) -> &mut Torrent {
        match self {
            Resource::Torrent(t) => t,
            _ => panic!(),
        }
    }

    pub fn as_file(&self) -> &File {
        match self {
            Resource::File(f) => f,
            _ => panic!(),
        }
    }

    pub fn as_piece(&self) -> &Piece {
        match self {
            Resource::Piece(p) => p,
            _ => panic!(),
        }
    }

    pub fn as_peer(&self) -> &Peer {
        match self {
            Resource::Peer(p) => p,
            _ => panic!(),
        }
    }

    pub fn as_tracker(&self) -> &Tracker {
        match self {
            Resource::Tracker(t) => t,
            _ => panic!(),
        }
    }

    pub fn update(&mut self, update: SResourceUpdate<'_>) {
        match self {
            Resource::Server(s) => {
                s.update(update);
            }
            Resource::Torrent(t) => {
                t.update(update);
            }
            Resource::Piece(p) => {
                p.update(update);
            }
            Resource::File(f) => {
                f.update(update);
            }
            Resource::Peer(p) => {
                p.update(update);
            }
            Resource::Tracker(t) => {
                t.update(update);
            }
        }
    }
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Resource::Server(t) => {
                writeln!(f, "Server {{")?;
                writeln!(f, "  id: {}", t.id)?;
                writeln!(f, "  upload: {} B/s", t.rate_up)?;
                writeln!(f, "  download: {} B/s", t.rate_down)?;
                match t.throttle_up {
                    Some(u) if u >= 0 => {
                        writeln!(f, "  throttle up: {} B/s", u)?;
                    }
                    Some(u) => {
                        writeln!(f, "  throttle up: invalid({})", u)?;
                    }
                    None => {
                        writeln!(f, "  throttle up: unlimited")?;
                    }
                }
                match t.throttle_down {
                    Some(u) if u >= 0 => {
                        writeln!(f, "  throttle down: {} B/s", u)?;
                    }
                    Some(u) => {
                        writeln!(f, "  throttle down: invalid({})", u)?;
                    }
                    None => {
                        writeln!(f, "  throttle down: unlimited")?;
                    }
                }
                writeln!(f, "  uploaded: {} B", t.transferred_up)?;
                writeln!(f, "  downloaded: {} B", t.transferred_down)?;
                writeln!(f, "  session upload: {} B", t.ses_transferred_up)?;
                writeln!(f, "  session download: {} B", t.ses_transferred_down)?;
                writeln!(f, "  started at: {}", t.started)?;
                write!(f, "}}")?;
            }
            Resource::Torrent(t) => {
                writeln!(f, "Torrent {{")?;
                writeln!(f, "  id: {}", t.id)?;
                writeln!(
                    f,
                    "  name: {}",
                    if let Some(ref n) = t.name {
                        n.as_str()
                    } else {
                        "Unknown (magnet)"
                    }
                )?;
                writeln!(f, "  path: {}", t.path)?;
                writeln!(f, "  created at: {}", t.created)?;
                writeln!(f, "  modified at: {}", t.modified)?;
                writeln!(f, "  status: {}", t.status.as_str())?;
                if let Some(ref e) = t.error {
                    writeln!(f, "  error: {}", e)?;
                }
                writeln!(f, "  priority: {}", t.priority)?;
                writeln!(f, "  progress: {}", t.progress)?;
                writeln!(f, "  availability: {}", t.availability)?;
                writeln!(f, "  strategy: {:?}", t.strategy)?;
                writeln!(f, "  upload: {} B/s", t.rate_up)?;
                writeln!(f, "  download: {} B/s", t.rate_down)?;
                match t.throttle_up {
                    Some(u) if u >= 0 => {
                        writeln!(f, "  throttle up: {} B/s", u)?;
                    }
                    Some(_) => {
                        writeln!(f, "  throttle up: unlimited")?;
                    }
                    None => {
                        writeln!(f, "  throttle up: server")?;
                    }
                }
                match t.throttle_down {
                    Some(u) if u >= 0 => {
                        writeln!(f, "  throttle down: {} B/s", u)?;
                    }
                    Some(_) => {
                        writeln!(f, "  throttle down: unlimited")?;
                    }
                    None => {
                        writeln!(f, "  throttle down: server")?;
                    }
                }
                writeln!(f, "  uploaded: {} B", t.transferred_up)?;
                writeln!(f, "  downloaded: {} B", t.transferred_down)?;
                writeln!(f, "  peers: {}", t.peers)?;
                writeln!(f, "  trackers: {}", t.trackers)?;
                if let Some(s) = t.size {
                    writeln!(f, "  size: {} B", s)?;
                } else {
                    writeln!(f, "  size: Unknown (magnet)")?;
                }
                if let Some(p) = t.pieces {
                    writeln!(f, "  pieces: {}", p)?;
                } else {
                    writeln!(f, "  pieces: Unknown (magnet)")?;
                }
                if let Some(p) = t.piece_size {
                    writeln!(f, "  piece size: {} B", p)?;
                } else {
                    writeln!(f, "  piece size: Unknown (magnet)")?;
                }
                if let Some(files) = t.files {
                    writeln!(f, "  files: {}", files)?;
                } else {
                    writeln!(f, "  files: Unknown (magnet)")?;
                }
                write!(f, "}}")?;
            }
            Resource::File(t) => {
                write!(f, "{:#?}", t)?;
            }
            Resource::Piece(t) => {
                write!(f, "{:#?}", t)?;
            }
            Resource::Peer(t) => {
                write!(f, "{:#?}", t)?;
            }
            Resource::Tracker(t) => {
                write!(f, "{:#?}", t)?;
            }
        }
        Ok(())
    }
}

fn deserialize_throttle<'de, D>(de: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let deser_result = serde::Deserialize::deserialize(de)?;
    match deser_result {
        json::Value::Null => Ok(Some(None)),
        json::Value::Number(ref i) if i.is_i64() => Ok(Some(Some(i.as_i64().unwrap()))),
        json::Value::Number(_) => Err(serde::de::Error::custom("Throttle must not be a float")),
        _ => Err(serde::de::Error::custom("Throttle must be number or null")),
    }
}

// TODO: Proc macros to remove this shit

impl Queryable for Resource {
    fn field(&self, f: &str) -> Option<Field<'_>> {
        match self {
            Resource::Server(t) => t.field(f),
            Resource::Torrent(t) => t.field(f),
            Resource::File(t) => t.field(f),
            Resource::Piece(t) => t.field(f),
            Resource::Peer(t) => t.field(f),
            Resource::Tracker(t) => t.field(f),
        }
    }
}

impl Queryable for json::Value {
    fn field(&self, f: &str) -> Option<Field<'_>> {
        match self.pointer(f) {
            Some(&json::Value::Null) => Some(FNULL),
            Some(&json::Value::Bool(b)) => Some(Field::B(b)),
            Some(json::Value::Number(n)) => {
                if n.is_f64() {
                    Some(Field::F(n.as_f64().unwrap() as f32))
                } else {
                    Some(Field::N(n.as_i64().unwrap()))
                }
            }
            Some(json::Value::String(s)) => Some(Field::S(s)),
            Some(json::Value::Array(a)) => {
                Some(Field::V(a.iter().filter_map(|v| v.field("")).collect()))
            }
            Some(json::Value::Object(_)) => None,
            None => None,
        }
    }
}

impl Queryable for Server {
    fn field(&self, f: &str) -> Option<Field<'_>> {
        match f {
            "id" => Some(Field::S(&self.id)),

            "rate_up" => Some(Field::N(self.rate_up as i64)),
            "rate_down" => Some(Field::N(self.rate_down as i64)),
            "throttle_up" => Some(self.throttle_up.map(Field::N).unwrap_or(FNULL)),
            "throttle_down" => Some(self.throttle_down.map(Field::N).unwrap_or(FNULL)),
            "transferred_up" => Some(Field::N(self.transferred_up as i64)),
            "transferred_down" => Some(Field::N(self.transferred_down as i64)),
            "ses_transferred_up" => Some(Field::N(self.ses_transferred_up as i64)),
            "ses_transferred_down" => Some(Field::N(self.ses_transferred_down as i64)),
            "free_space" => Some(Field::N(self.free_space as i64)),

            "started" => Some(Field::D(self.started)),

            _ if f.starts_with("user_data") => self.user_data.field(&f[9..]),

            _ => None,
        }
    }
}

impl Queryable for Torrent {
    fn field(&self, f: &str) -> Option<Field<'_>> {
        match f {
            "id" => Some(Field::S(&self.id)),
            "name" => Some(
                self.name
                    .as_ref()
                    .map(|v| Field::S(v.as_str()))
                    .unwrap_or(FNULL),
            ),
            "private" => Some(Field::B(self.private)),
            "creator" => Some(
                self.creator
                    .as_ref()
                    .map(|v| Field::S(v.as_str()))
                    .unwrap_or(FNULL),
            ),
            "comment" => Some(
                self.comment
                    .as_ref()
                    .map(|v| Field::S(v.as_str()))
                    .unwrap_or(FNULL),
            ),
            "path" => Some(Field::S(&self.path)),
            "status" => Some(Field::S(self.status.as_str())),
            "error" => Some(
                self.error
                    .as_ref()
                    .map(|v| Field::S(v.as_str()))
                    .unwrap_or(FNULL),
            ),

            "priority" => Some(Field::N(self.priority as i64)),
            "rate_up" => Some(Field::N(self.rate_up as i64)),
            "rate_down" => Some(Field::N(self.rate_down as i64)),
            "throttle_up" => Some(self.throttle_up.map(Field::N).unwrap_or(FNULL)),
            "throttle_down" => Some(self.throttle_down.map(Field::N).unwrap_or(FNULL)),
            "transferred_up" => Some(Field::N(self.transferred_up as i64)),
            "transferred_down" => Some(Field::N(self.transferred_down as i64)),
            "peers" => Some(Field::N(self.peers as i64)),
            "trackers" => Some(Field::N(self.trackers as i64)),
            "tracker_urls" => Some(Field::V(
                self.tracker_urls.iter().map(|url| Field::S(url)).collect(),
            )),
            "size" => Some(self.size.map(|v| Field::N(v as i64)).unwrap_or(FNULL)),
            "pieces" => Some(self.pieces.map(|v| Field::N(v as i64)).unwrap_or(FNULL)),
            "piece_size" => Some(self.piece_size.map(|v| Field::N(v as i64)).unwrap_or(FNULL)),
            "files" => Some(self.files.map(|v| Field::N(v as i64)).unwrap_or(FNULL)),

            "created" => Some(Field::D(self.created)),
            "modified" => Some(Field::D(self.modified)),

            "progress" => Some(Field::F(self.progress)),
            "availability" => Some(Field::F(self.availability)),

            "strategy" => Some(Field::S(self.strategy.as_str())),

            _ if f.starts_with("user_data") => self.user_data.field(&f[9..]),

            _ if f.starts_with("tracker/") => Some(Field::R(ResourceKind::Tracker)),
            _ if f.starts_with("file/") => Some(Field::R(ResourceKind::File)),
            _ if f.starts_with("peer/") => Some(Field::R(ResourceKind::Peer)),

            _ => None,
        }
    }
}

impl Queryable for Piece {
    fn field(&self, f: &str) -> Option<Field<'_>> {
        match f {
            "id" => Some(Field::S(&self.id)),
            "torrent_id" => Some(Field::S(&self.torrent_id)),

            "available" => Some(Field::B(self.available)),
            "downloaded" => Some(Field::B(self.downloaded)),

            _ if f.starts_with("user_data") => self.user_data.field(&f[9..]),

            _ => None,
        }
    }
}

impl Queryable for File {
    fn field(&self, f: &str) -> Option<Field<'_>> {
        match f {
            "id" => Some(Field::S(&self.id)),
            "torrent_id" => Some(Field::S(&self.torrent_id)),
            "path" => Some(Field::S(&self.path)),

            "priority" => Some(Field::N(self.priority as i64)),

            "progress" => Some(Field::F(self.progress)),

            _ if f.starts_with("user_data") => self.user_data.field(&f[9..]),

            _ => None,
        }
    }
}

impl Queryable for Peer {
    fn field(&self, f: &str) -> Option<Field<'_>> {
        match f {
            "id" => Some(Field::S(&self.id)),
            "torrent_id" => Some(Field::S(&self.torrent_id)),
            "ip" => Some(Field::S(&self.ip)),

            "rate_up" => Some(Field::N(self.rate_up as i64)),
            "rate_down" => Some(Field::N(self.rate_down as i64)),

            "availability" => Some(Field::F(self.availability)),

            "client_id" => Some(Field::S(&self.client_id)),

            _ if f.starts_with("user_data") => self.user_data.field(&f[9..]),

            _ => None,
        }
    }
}

impl Queryable for Tracker {
    fn field(&self, f: &str) -> Option<Field<'_>> {
        match f {
            "id" => Some(Field::S(&self.id)),
            "torrent_id" => Some(Field::S(&self.torrent_id)),
            "url" => Some(Field::S(self.url.as_str())),
            "error" => Some(
                self.error
                    .as_ref()
                    .map(|v| Field::S(v.as_str()))
                    .unwrap_or(FNULL),
            ),

            "last_report" => Some(Field::D(self.last_report)),

            _ if f.starts_with("user_data") => self.user_data.field(&f[9..]),

            _ => None,
        }
    }
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match *self {
            Status::Pending => "pending",
            Status::Paused => "paused",
            Status::Leeching => "leeching",
            Status::Idle => "idle",
            Status::Seeding => "seeding",
            Status::Hashing => "hashing",
            Status::Magnet => "magnet",
            Status::Error => "error",
        }
    }
}

/// Merges json objects according to RFC 7396
pub fn merge_json(original: &mut json::Value, update: &mut json::Value) {
    match (original, update) {
        (&mut json::Value::Object(ref mut o), &mut json::Value::Object(ref mut u)) => {
            for (k, v) in u.iter_mut() {
                if v.is_null() {
                    o.remove(k);
                } else if o.contains_key(k) {
                    merge_json(o.get_mut(k).unwrap(), v);
                } else {
                    o.insert(k.to_owned(), mem::replace(v, json::Value::Null));
                }
            }
        }
        (o, u) => {
            mem::swap(o, u);
        }
    }
}

impl Default for Server {
    fn default() -> Self {
        Server {
            id: "".to_owned(),
            rate_up: 0,
            rate_down: 0,
            throttle_up: None,
            throttle_down: None,
            transferred_up: 0,
            transferred_down: 0,
            ses_transferred_up: 0,
            ses_transferred_down: 0,
            free_space: 0,
            download_token: "".to_owned(),
            started: Utc::now(),
            user_data: json::Value::Null,
        }
    }
}

impl Default for Torrent {
    fn default() -> Self {
        Torrent {
            id: "".to_owned(),
            name: None,
            comment: None,
            creator: None,
            private: false,
            path: "".to_owned(),
            created: Utc::now(),
            modified: Utc::now(),
            status: Default::default(),
            error: None,
            priority: 0,
            progress: 0.,
            availability: 0.,
            strategy: Strategy::Rarest,
            rate_up: 0,
            rate_down: 0,
            throttle_up: None,
            throttle_down: None,
            transferred_up: 0,
            transferred_down: 0,
            peers: 0,
            trackers: 0,
            tracker_urls: vec![],
            size: None,
            pieces: None,
            piece_size: None,
            piece_field: "".to_owned(),
            files: None,
            user_data: json::Value::Null,
        }
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Tracker {
            id: "".to_owned(),
            torrent_id: "".to_owned(),
            url: Url::parse("http://my.tracker/announce").unwrap(),
            last_report: Utc::now(),
            error: None,
            user_data: json::Value::Null,
        }
    }
}
