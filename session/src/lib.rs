#[macro_use]
extern crate serde_derive;

pub mod torrent {
    pub use self::current::Session;
    pub use self::ver_fa1b6f as current;

    #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
    pub struct Bitfield {
        pub len: u64,
        pub data: Box<[u8]>,
    }

    pub fn load(data: &[u8]) -> Option<Session> {
        if let Ok(m) = bincode::deserialize::<ver_fa1b6f::Session>(data) {
            Some(m)
        } else if let Ok(m) = bincode::deserialize::<ver_6e27af::Session>(data) {
            Some(m.migrate())
        } else if let Ok(m) = bincode::deserialize::<ver_249b1b::Session>(data) {
            Some(m.migrate())
        } else if let Ok(m) = bincode::deserialize::<ver_5f166d::Session>(data) {
            Some(m.migrate())
        } else if let Ok(m) = bincode::deserialize::<ver_8e1121::Session>(data) {
            Some(m.migrate())
        } else {
            None
        }
    }

    impl Session {
        pub fn migrate(self) -> Self {
            self
        }
    }

    pub mod ver_fa1b6f {
        use super::Bitfield;

        use chrono::{DateTime, Utc};

        use std::path::PathBuf;

        #[derive(Deserialize, Debug, PartialEq, Serialize)]
        pub struct Session {
            pub info: Info,
            pub pieces: Bitfield,
            pub uploaded: u64,
            pub downloaded: u64,
            pub status: Status,
            pub path: Option<String>,
            pub priority: u8,
            pub priorities: Vec<u8>,
            pub created: DateTime<Utc>,
            pub throttle_ul: Option<i64>,
            pub throttle_dl: Option<i64>,
            pub trackers: Vec<String>,
        }

        #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
        pub struct Info {
            pub name: String,
            pub announce: Option<String>,
            pub creator: Option<String>,
            pub comment: Option<String>,
            pub piece_len: u32,
            pub total_len: u64,
            pub hashes: Vec<Vec<u8>>,
            pub hash: [u8; 20],
            pub files: Vec<File>,
            pub private: bool,
            pub be_name: Option<Vec<u8>>,
            pub piece_idx: Vec<(usize, u64)>,
        }

        #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
        pub struct File {
            pub path: PathBuf,
            pub length: u64,
        }

        #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
        pub struct Status {
            pub paused: bool,
            pub validating: bool,
            pub error: Option<String>,
            pub state: StatusState,
        }

        #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
        pub enum StatusState {
            Magnet,
            // Torrent has not acquired all pieces
            Incomplete,
            // Torrent has acquired all pieces, regardless of validity
            Complete,
        }
    }

    pub mod ver_6e27af {
        pub use self::next::{File, Status, StatusState};
        pub use super::ver_fa1b6f as next;

        use super::Bitfield;

        use chrono::{DateTime, Utc};

        #[derive(Serialize, Deserialize)]
        pub struct Session {
            pub info: Info,
            pub pieces: Bitfield,
            pub uploaded: u64,
            pub downloaded: u64,
            pub status: Status,
            pub path: Option<String>,
            pub priority: u8,
            pub priorities: Vec<u8>,
            pub created: DateTime<Utc>,
            pub throttle_ul: Option<i64>,
            pub throttle_dl: Option<i64>,
            pub trackers: Vec<String>,
        }

        #[derive(Clone, Serialize, Deserialize)]
        pub struct Info {
            pub name: String,
            pub announce: Option<String>,
            pub piece_len: u32,
            pub total_len: u64,
            pub hashes: Vec<Vec<u8>>,
            pub hash: [u8; 20],
            pub files: Vec<File>,
            pub private: bool,
            pub be_name: Option<Vec<u8>>,
            pub piece_idx: Vec<(usize, u64)>,
        }

        impl Session {
            pub fn migrate(self) -> super::current::Session {
                next::Session {
                    info: next::Info {
                        comment: None,
                        creator: None,
                        name: self.info.name,
                        announce: self.info.announce,
                        piece_len: self.info.piece_len,
                        total_len: self.info.total_len,
                        hashes: self.info.hashes,
                        hash: self.info.hash,
                        files: self.info.files,
                        private: self.info.private,
                        be_name: self.info.be_name,
                        piece_idx: self.info.piece_idx,
                    },
                    pieces: self.pieces,
                    uploaded: self.uploaded,
                    downloaded: self.downloaded,
                    status: self.status,
                    path: self.path,
                    priority: self.priority,
                    priorities: self.priorities,
                    created: self.created,
                    throttle_ul: self.throttle_ul,
                    throttle_dl: self.throttle_dl,
                    trackers: self.trackers,
                }
                .migrate()
            }
        }
    }

    pub mod ver_249b1b {
        pub use self::next::{File, Info, Status, StatusState};
        pub use super::ver_6e27af as next;
        use super::Bitfield;

        use chrono::{DateTime, Utc};

        #[derive(Serialize, Deserialize)]
        pub struct Session {
            pub info: Info,
            pub pieces: Bitfield,
            pub uploaded: u64,
            pub downloaded: u64,
            pub status: Status,
            pub path: Option<String>,
            pub priority: u8,
            pub priorities: Vec<u8>,
            pub created: DateTime<Utc>,
            pub throttle_ul: Option<i64>,
            pub throttle_dl: Option<i64>,
        }

        impl Session {
            pub fn migrate(self) -> super::current::Session {
                let mut trackers = Vec::new();
                if let Some(ref url) = self.info.announce {
                    trackers.push(url.to_owned());
                }
                next::Session {
                    info: self.info,
                    pieces: self.pieces,
                    uploaded: self.uploaded,
                    downloaded: self.downloaded,
                    status: self.status,
                    path: self.path,
                    priority: self.priority,
                    priorities: self.priorities,
                    created: self.created,
                    throttle_ul: self.throttle_ul,
                    throttle_dl: self.throttle_dl,
                    trackers,
                }
                .migrate()
            }
        }
    }

    pub mod ver_5f166d {
        use super::ver_249b1b as next;
        use super::Bitfield;

        use chrono::{DateTime, Utc};

        #[derive(Serialize, Deserialize)]
        pub struct Session {
            pub info: Info,
            pub pieces: Bitfield,
            pub uploaded: u64,
            pub downloaded: u64,
            pub status: Status,
            pub path: Option<String>,
            pub priority: u8,
            pub priorities: Vec<u8>,
            pub created: DateTime<Utc>,
            pub throttle_ul: Option<i64>,
            pub throttle_dl: Option<i64>,
        }

        #[derive(Serialize, Deserialize)]
        pub enum Status {
            Pending,
            Paused,
            Leeching,
            Idle,
            Seeding,
            Validating,
            Magnet,
            DiskError,
        }

        #[derive(Serialize, Deserialize)]
        pub struct Info {
            pub name: String,
            pub announce: String,
            pub piece_len: u32,
            pub total_len: u64,
            pub hashes: Vec<Vec<u8>>,
            pub hash: [u8; 20],
            pub files: Vec<next::File>,
            pub private: bool,
            pub be_name: Option<Vec<u8>>,
        }

        impl Session {
            pub fn migrate(self) -> super::current::Session {
                let mut state = next::StatusState::Complete;
                for i in 0..self.pieces.len - 1 {
                    if !(self.pieces.data[i as usize]) != 0 {
                        state = next::StatusState::Incomplete;
                        break;
                    }
                }
                if !self.pieces.data.is_empty() {
                    match (self.pieces.len % 8, *self.pieces.data.last().unwrap()) {
                        (0, 0xFF)
                        | (7, 0xFE)
                        | (6, 0xFC)
                        | (5, 0xF8)
                        | (4, 0xF0)
                        | (3, 0xE0)
                        | (2, 0xC0)
                        | (1, 0x80) => {}
                        _ => state = next::StatusState::Incomplete,
                    }
                }
                let paused = matches!(self.status, Status::Paused);
                let piece_idx = generate_piece_idx(
                    self.info.hashes.len(),
                    self.info.piece_len as u64,
                    &self.info.files,
                );
                next::Session {
                    info: next::Info {
                        name: self.info.name,
                        announce: if self.info.announce.is_empty() {
                            None
                        } else {
                            Some(self.info.announce)
                        },
                        piece_len: self.info.piece_len,
                        total_len: self.info.total_len,
                        hashes: self.info.hashes,
                        hash: self.info.hash,
                        files: self.info.files,
                        private: self.info.private,
                        be_name: self.info.be_name,
                        piece_idx,
                    },
                    pieces: self.pieces,
                    uploaded: self.uploaded,
                    downloaded: self.downloaded,
                    status: next::Status {
                        paused,
                        state,
                        validating: false,
                        error: None,
                    },
                    path: self.path,
                    priority: self.priority,
                    priorities: self.priorities,
                    created: self.created,
                    throttle_ul: self.throttle_ul,
                    throttle_dl: self.throttle_dl,
                }
                .migrate()
            }
        }

        fn generate_piece_idx(pieces: usize, pl: u64, files: &[next::File]) -> Vec<(usize, u64)> {
            let mut piece_idx = Vec::with_capacity(pieces);
            let mut file = 0;
            let mut offset = 0u64;
            for _ in 0..pieces {
                piece_idx.push((file, offset));
                offset += pl;
                while file < files.len() && offset >= files[file].length {
                    offset -= files[file].length;
                    file += 1;
                }
            }
            piece_idx
        }
    }

    pub mod ver_8e1121 {
        use self::next::{Info, Status};
        use super::ver_5f166d as next;
        use super::Bitfield;

        use chrono::{DateTime, Utc};

        #[derive(Serialize, Deserialize)]
        pub struct Session {
            pub info: Info,
            pub pieces: Bitfield,
            pub uploaded: u64,
            pub downloaded: u64,
            pub status: Status,
            pub path: Option<String>,
            pub wanted: Bitfield,
            pub priority: u8,
            pub priorities: Vec<u8>,
            pub created: DateTime<Utc>,
            pub throttle_ul: Option<i64>,
            pub throttle_dl: Option<i64>,
        }

        impl Session {
            pub fn migrate(self) -> super::current::Session {
                next::Session {
                    info: self.info,
                    pieces: self.pieces,
                    uploaded: self.uploaded,
                    downloaded: self.downloaded,
                    status: self.status,
                    path: self.path,
                    priority: self.priority,
                    priorities: self.priorities,
                    created: self.created,
                    throttle_ul: self.throttle_ul,
                    throttle_dl: self.throttle_dl,
                }
                .migrate()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use std::path::PathBuf;

    use super::torrent::current::{File, Info, Session, Status, StatusState};
    use super::torrent::Bitfield;

    fn session_instance() -> Session {
        Session {
            info: Info {
                name: "Hello world!".to_string(),
                announce: Some("announce".to_string()),
                creator: Some("creator".to_string()),
                comment: Some("comment".to_string()),
                piece_len: 1048576,
                total_len: 2 * 1048576,
                hashes: vec![
                    b"\x20\x21\x22\x23\x24\x25\x26\x27\x28\x29\x20\x21\x22\x23\x24\x25\x26\x27\x28\x29".to_vec(),
                    b"\x30\x31\x32\x33\x34\x35\x36\x37\x38\x39\x30\x31\x32\x33\x34\x35\x36\x37\x38\x39".to_vec(),
                ],
                hash: *b"\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19",
                files: vec![
                    File {
                        path: PathBuf::from("file1"),
                        length: 1024,
                    },
                    File {
                        path: PathBuf::from("file2"),
                        length: 2 * 1048576 - 1024,
                    },
                ],
                private: true,
                be_name: None,
                piece_idx: vec![(0, 1), (1, 0)],
            },
            pieces: Bitfield {
                len: 2,
                data: Box::new([3]),
            },
            uploaded: 7777777,
            downloaded: 88888888,
            status: Status {
                paused: false,
                validating: true,
                error: Some("an error".to_string()),
                state: StatusState::Complete,
            },
            path: Some("/tmp".to_string()),
            priority: 100,
            priorities: vec![],
            created: DateTime::from_timestamp(946684799, 0).unwrap(),
            throttle_ul: Some(64 * 1024 * 1024),
            throttle_dl: None,
            trackers: vec!["https://example.com:1234/tracker".to_string()],
        }
    }

    const SESSION_SERIALIZATION: &[u8] = &[
        12, 0, 0, 0, 0, 0, 0, 0, 72, 101, 108, 108, 111, 32, 119, 111, 114, 108, 100, 33, 1, 8, 0,
        0, 0, 0, 0, 0, 0, 97, 110, 110, 111, 117, 110, 99, 101, 1, 7, 0, 0, 0, 0, 0, 0, 0, 99, 114,
        101, 97, 116, 111, 114, 1, 7, 0, 0, 0, 0, 0, 0, 0, 99, 111, 109, 109, 101, 110, 116, 0, 0,
        16, 0, 0, 0, 32, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 20, 0, 0, 0, 0, 0, 0, 0, 32, 33,
        34, 35, 36, 37, 38, 39, 40, 41, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 20, 0, 0, 0, 0, 0,
        0, 0, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 16,
        17, 18, 19, 20, 21, 22, 23, 24, 25, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 2, 0, 0, 0, 0,
        0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 102, 105, 108, 101, 49, 0, 4, 0, 0, 0, 0, 0, 0, 5, 0, 0,
        0, 0, 0, 0, 0, 102, 105, 108, 101, 50, 0, 252, 31, 0, 0, 0, 0, 0, 1, 0, 2, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3, 241, 173, 118, 0, 0, 0, 0,
        0, 56, 86, 76, 5, 0, 0, 0, 0, 0, 1, 1, 8, 0, 0, 0, 0, 0, 0, 0, 97, 110, 32, 101, 114, 114,
        111, 114, 2, 0, 0, 0, 1, 4, 0, 0, 0, 0, 0, 0, 0, 47, 116, 109, 112, 100, 0, 0, 0, 0, 0, 0,
        0, 0, 20, 0, 0, 0, 0, 0, 0, 0, 49, 57, 57, 57, 45, 49, 50, 45, 51, 49, 84, 50, 51, 58, 53,
        57, 58, 53, 57, 90, 1, 0, 0, 0, 4, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 0,
        0, 0, 0, 104, 116, 116, 112, 115, 58, 47, 47, 101, 120, 97, 109, 112, 108, 101, 46, 99,
        111, 109, 58, 49, 50, 51, 52, 47, 116, 114, 97, 99, 107, 101, 114,
    ];

    #[test]
    fn stable_deserialize() {
        assert_eq!(
            bincode::serialize(&session_instance()).unwrap(),
            SESSION_SERIALIZATION
        );
    }

    #[test]
    fn stable_serialize() {
        assert_eq!(
            super::torrent::load(&SESSION_SERIALIZATION).unwrap(),
            session_instance()
        );
    }
}
