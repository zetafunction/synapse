use std::io::{self, Read};
use std::mem;

use byteorder::{BigEndian, ByteOrder};

use crate::buffers::{Buffer, BUF_SIZE};
use crate::torrent::peer::Message;
use crate::torrent::Bitfield;
use crate::util::{aread, io_err_val, IOR};

const MAX_EXT_MSG_BYTES: u32 = 100 * 1000 * 1000;

pub struct Reader {
    state: State,
    prefix: [u8; 17],
    idx: usize,
}

enum State {
    Len,
    ID,
    Have,
    Request,
    Cancel,
    Port,
    Handshake { data: [u8; 68] },
    PiecePrefix,
    Piece { data: Option<Buffer>, len: u32 },
    Bitfield { data: Vec<u8> },
    ExtensionID,
    Extension { id: u8, payload: Vec<u8> },
}

#[derive(Debug)]
pub enum RRes {
    Success(Message),
    Err(io::Error),
    Blocked,
    Stalled,
}

#[cfg(test)]
impl RRes {
    fn unwrap(self) -> Option<Message> {
        match self {
            RRes::Success(m) => Some(m),
            _ => None,
        }
    }
}

impl Reader {
    pub fn new() -> Reader {
        Reader {
            prefix: [0u8; 17],
            idx: 0,
            state: State::Handshake { data: [0u8; 68] },
        }
    }

    pub fn readable<R: Read>(&mut self, conn: &mut R) -> RRes {
        let res = self.readable_(conn);
        if let RRes::Success(_) = &res {
            self.state = State::Len;
            self.idx = 0;
        }
        res
    }

    fn readable_<R: Read>(&mut self, conn: &mut R) -> RRes {
        loop {
            let len = self.state.len();
            match self.state {
                State::Handshake { ref mut data } => match aread(&mut data[self.idx..len], conn) {
                    IOR::Complete => {
                        if &data[1..20] != b"BitTorrent protocol" {
                            return RRes::Err(io_err_val(
                                "Handshake was not for 'BitTorrent protocol'",
                            ));
                        }
                        let mut rsv = [0; 8];
                        rsv.clone_from_slice(&data[20..28]);
                        let mut hash = [0; 20];
                        hash.clone_from_slice(&data[28..48]);
                        let mut id = [0; 20];
                        id.clone_from_slice(&data[48..68]);

                        return RRes::Success(Message::Handshake { rsv, hash, id });
                    }
                    IOR::Incomplete(a) => self.idx += a,
                    IOR::Blocked => return RRes::Blocked,
                    IOR::EOF => return RRes::Err(io_err_val("EOF")),
                    IOR::Err(e) => return RRes::Err(e),
                },
                State::Len => match aread(&mut self.prefix[self.idx..len], conn) {
                    IOR::Complete => {
                        let mlen = BigEndian::read_u32(&self.prefix[0..4]);
                        if mlen == 0 {
                            return RRes::Success(Message::KeepAlive);
                        } else {
                            self.idx = 4;
                            self.state = State::ID;
                        }
                    }
                    IOR::Incomplete(a) => self.idx += a,
                    IOR::Blocked => return RRes::Blocked,
                    IOR::EOF => return RRes::Err(io_err_val("EOF")),
                    IOR::Err(e) => return RRes::Err(e),
                },
                State::ID => match aread(&mut self.prefix[self.idx..len], conn) {
                    IOR::Complete => {
                        self.idx = 5;
                        match self.prefix[4] {
                            0..=3 => {
                                let id = self.prefix[4];
                                let msg = if id == 0 {
                                    Message::Choke
                                } else if id == 1 {
                                    Message::Unchoke
                                } else if id == 2 {
                                    Message::Interested
                                } else {
                                    Message::Uninterested
                                };
                                return RRes::Success(msg);
                            }
                            4 => self.state = State::Have,
                            5 => {
                                let mlen = BigEndian::read_u32(&self.prefix[0..4]);
                                if mlen as usize > BUF_SIZE {
                                    // we'll check the exact length later
                                    return RRes::Err(io::Error::new(
                                        io::ErrorKind::Other,
                                        format!("Invalid bitfield length {}", mlen),
                                    ));
                                }
                                self.idx = 0;
                                self.state = State::Bitfield {
                                    data: vec![0u8; mlen as usize - 1],
                                };
                            }
                            6 => self.state = State::Request,
                            7 => self.state = State::PiecePrefix,
                            8 => self.state = State::Cancel,
                            9 => self.state = State::Port,
                            20 => self.state = State::ExtensionID,
                            _ => return RRes::Err(io_err_val("Invalid ID used!")),
                        }
                    }
                    IOR::Blocked => return RRes::Blocked,
                    IOR::EOF => return RRes::Err(io_err_val("EOF")),
                    IOR::Err(e) => return RRes::Err(e),
                    IOR::Incomplete(_) => unreachable!(),
                },
                State::Have => match aread(&mut self.prefix[self.idx..len], conn) {
                    IOR::Complete => {
                        let have = BigEndian::read_u32(&self.prefix[5..9]);
                        return RRes::Success(Message::Have(have));
                    }
                    IOR::Incomplete(a) => self.idx += a,
                    IOR::Blocked => return RRes::Blocked,
                    IOR::EOF => return RRes::Err(io_err_val("EOF")),
                    IOR::Err(e) => return RRes::Err(e),
                },
                State::Bitfield { ref mut data } => match aread(&mut data[self.idx..len], conn) {
                    IOR::Complete => {
                        let d = mem::take(data).into_boxed_slice();
                        let bf = Bitfield::from(&d, len as u64 * 8);
                        return RRes::Success(Message::Bitfield(bf));
                    }
                    IOR::Incomplete(a) => self.idx += a,
                    IOR::Blocked => return RRes::Blocked,
                    IOR::EOF => return RRes::Err(io_err_val("EOF")),
                    IOR::Err(e) => return RRes::Err(e),
                },
                State::Request => match aread(&mut self.prefix[self.idx..len], conn) {
                    IOR::Complete => {
                        let index = BigEndian::read_u32(&self.prefix[5..9]);
                        let begin = BigEndian::read_u32(&self.prefix[9..13]);
                        let length = BigEndian::read_u32(&self.prefix[13..17]);
                        return RRes::Success(Message::Request {
                            index,
                            begin,
                            length,
                        });
                    }
                    IOR::Incomplete(a) => self.idx += a,
                    IOR::Blocked => return RRes::Blocked,
                    IOR::EOF => return RRes::Err(io_err_val("EOF")),
                    IOR::Err(e) => return RRes::Err(e),
                },
                State::PiecePrefix => match aread(&mut self.prefix[self.idx..len], conn) {
                    IOR::Complete => {
                        let plen = BigEndian::read_u32(&self.prefix[0..4]) - 9;
                        if plen as usize > BUF_SIZE {
                            return RRes::Err(io::Error::new(
                                io::ErrorKind::Other,
                                format!("Invalid pieces length {}", plen),
                            ));
                        }
                        self.idx = 0;
                        self.state = State::Piece {
                            data: Buffer::get(),
                            len: plen,
                        };
                    }
                    IOR::Incomplete(a) => self.idx += a,
                    IOR::Blocked => return RRes::Blocked,
                    IOR::EOF => return RRes::Err(io_err_val("EOF")),
                    IOR::Err(e) => return RRes::Err(e),
                },
                State::Piece {
                    ref mut data,
                    len: length,
                } => {
                    if data.is_none() {
                        if let Some(buf) = Buffer::get() {
                            *data = Some(buf);
                        } else {
                            return RRes::Stalled;
                        }
                    }
                    match aread(&mut data.as_mut().unwrap()[self.idx..len], conn) {
                        IOR::Complete => {
                            let index = BigEndian::read_u32(&self.prefix[5..9]);
                            let begin = BigEndian::read_u32(&self.prefix[9..13]);
                            return RRes::Success(Message::Piece {
                                index,
                                begin,
                                length,
                                data: data.take().unwrap(),
                            });
                        }
                        IOR::Incomplete(a) => self.idx += a,
                        IOR::Blocked => return RRes::Blocked,
                        IOR::EOF => return RRes::Err(io_err_val("EOF")),
                        IOR::Err(e) => return RRes::Err(e),
                    }
                }
                State::Cancel => match aread(&mut self.prefix[self.idx..len], conn) {
                    IOR::Complete => {
                        let index = BigEndian::read_u32(&self.prefix[5..9]);
                        let begin = BigEndian::read_u32(&self.prefix[9..13]);
                        let length = BigEndian::read_u32(&self.prefix[13..17]);
                        return RRes::Success(Message::Cancel {
                            index,
                            begin,
                            length,
                        });
                    }
                    IOR::Incomplete(a) => self.idx += a,
                    IOR::Blocked => return RRes::Blocked,
                    IOR::EOF => return RRes::Err(io_err_val("EOF")),
                    IOR::Err(e) => return RRes::Err(e),
                },
                State::Port => match aread(&mut self.prefix[self.idx..len], conn) {
                    IOR::Complete => {
                        let port = BigEndian::read_u16(&self.prefix[5..7]);
                        return RRes::Success(Message::Port(port));
                    }
                    IOR::Incomplete(a) => self.idx += a,
                    IOR::Blocked => return RRes::Blocked,
                    IOR::EOF => return RRes::Err(io_err_val("EOF")),
                    IOR::Err(e) => return RRes::Err(e),
                },
                State::ExtensionID => match aread(&mut self.prefix[5..6], conn) {
                    IOR::Complete => {
                        let id = self.prefix[5];
                        self.idx = 0;
                        let plen = BigEndian::read_u32(&self.prefix[0..4]) - 2;
                        if plen > MAX_EXT_MSG_BYTES {
                            return RRes::Err(io_err_val("Ext message too large"));
                        }
                        let payload = vec![0u8; plen as usize];
                        self.state = State::Extension { id, payload };
                    }
                    IOR::Blocked => return RRes::Blocked,
                    IOR::EOF => return RRes::Err(io_err_val("EOF")),
                    IOR::Err(e) => return RRes::Err(e),
                    IOR::Incomplete(_) => unreachable!(),
                },
                State::Extension {
                    id,
                    ref mut payload,
                } => match aread(&mut payload[self.idx..len], conn) {
                    IOR::Complete => {
                        let p = mem::replace(payload, Vec::with_capacity(0));
                        return RRes::Success(Message::Extension { id, payload: p });
                    }
                    IOR::Incomplete(a) => self.idx += a,
                    IOR::Blocked => return RRes::Blocked,
                    IOR::EOF => return RRes::Err(io_err_val("EOF")),
                    IOR::Err(e) => return RRes::Err(e),
                },
            }
        }
    }
}

impl State {
    fn len(&self) -> usize {
        match *self {
            State::Len => 4,
            State::ID => 5,
            State::Have => 9,
            State::Request | State::Cancel => 17,
            State::PiecePrefix => 13,
            State::Port => 7,
            State::Handshake { .. } => 68,
            State::Piece { len, .. } => len as usize,
            State::Bitfield { ref data, .. } => data.len(),
            State::ExtensionID => 6,
            State::Extension { ref payload, .. } => payload.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::torrent::peer::Message;
    use std::io::{self, Read};

    /// Cursor to emulate a mio socket using readv.
    struct Cursor<'a> {
        data: &'a [u8],
        idx: usize,
    }

    impl<'a> Cursor<'a> {
        fn new(data: &'a [u8]) -> Cursor<'_> {
            Cursor { data, idx: 0 }
        }
    }

    impl<'a> Read for Cursor<'a> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.idx >= self.data.len() {
                return Err(io::Error::new(io::ErrorKind::WouldBlock, ""));
            }
            let start = self.idx;
            for i in 0..buf.len() {
                if self.idx >= self.data.len() {
                    break;
                }
                buf[i] = self.data[self.idx];
                self.idx += 1;
            }
            Ok(self.idx - start)
        }
    }

    fn test_message(data: Vec<u8>, msg: Message) {
        let mut r = Reader::new();
        r.state = State::Len;
        let mut data = Cursor::new(&data);
        assert_eq!(msg, r.readable(&mut data).unwrap().unwrap())
    }

    #[test]
    fn test_read_keepalive() {
        let mut r = Reader::new();
        r.state = State::Len;
        let data = vec![0u8, 0, 0, 0];
        test_message(data, Message::KeepAlive);
    }

    #[test]
    fn test_read_choke() {
        let mut r = Reader::new();
        r.state = State::Len;
        let data = vec![0u8, 0, 0, 1, 0];
        test_message(data, Message::Choke);
    }

    #[test]
    fn test_read_unchoke() {
        let mut r = Reader::new();
        r.state = State::Len;
        let data = vec![0u8, 0, 0, 1, 1];
        test_message(data, Message::Unchoke);
    }

    #[test]
    fn test_read_interested() {
        let mut r = Reader::new();
        r.state = State::Len;
        let data = vec![0u8, 0, 0, 1, 2];
        test_message(data, Message::Interested);
    }

    #[test]
    fn test_read_uninterested() {
        let mut r = Reader::new();
        r.state = State::Len;
        let data = vec![0u8, 0, 0, 1, 3];
        test_message(data, Message::Uninterested);
    }

    #[test]
    fn test_read_have() {
        let mut r = Reader::new();
        r.state = State::Len;
        let data = vec![0u8, 0, 0, 5, 4, 0, 0, 0, 1];
        test_message(data, Message::Have(1));
    }

    #[test]
    fn test_read_bitfield() {
        let mut r = Reader::new();
        r.state = State::Len;
        let v = vec![0u8, 0, 0, 5, 5, 0xff, 0xff, 0xff, 0xff];
        let mut data = Cursor::new(&v);
        // Test one shot
        match r.readable(&mut data).unwrap().unwrap() {
            Message::Bitfield(ref pf) => {
                for i in 0..32 {
                    assert!(pf.has_bit(i as u64));
                }
            }
            _ => {
                unreachable!();
            }
        }
    }

    #[test]
    fn test_read_request() {
        let mut r = Reader::new();
        r.state = State::Len;
        let v = vec![0u8, 0, 0, 13, 6, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1];
        let mut data = Cursor::new(&v);
        // Test one shot
        match r.readable(&mut data).unwrap().unwrap() {
            Message::Request {
                index,
                begin,
                length,
            } => {
                assert_eq!(index, 1);
                assert_eq!(begin, 1);
                assert_eq!(length, 1);
            }
            _ => {
                unreachable!();
            }
        }
    }

    #[test]
    fn test_read_piece() {
        let mut r = Reader::new();
        r.state = State::Len;
        let mut v = vec![0u8, 0, 0x40, 0x09, 7, 0, 0, 0, 1, 0, 0, 0, 1];
        v.extend(vec![1u8; 16_384]);
        v.extend(vec![0u8, 0, 0x40, 0x09, 7, 0, 0, 0, 1, 0, 0, 0, 1]);
        v.extend(vec![1u8; 16_384]);

        let mut p1 = Cursor::new(&v[0..10]);
        let mut p2 = Cursor::new(&v[10..100]);
        let mut p3 = Cursor::new(&v[100..]);
        // Test partial read
        assert_eq!(r.readable(&mut p1).unwrap(), None);
        assert_eq!(r.readable(&mut p2).unwrap(), None);
        match r.readable(&mut p3) {
            RRes::Success(Message::Piece {
                index,
                begin,
                length,
                ref data,
            }) => {
                assert_eq!(index, 1);
                assert_eq!(begin, 1);
                assert_eq!(length, 16_384);
                for i in 0..16_384 {
                    assert_eq!(1, data[i]);
                }
            }
            res => {
                panic!("Failed to get piece: {:?}", res);
            }
        }
    }

    #[test]
    fn test_read_cancel() {
        let mut r = Reader::new();
        r.state = State::Len;
        let v = vec![0u8, 0, 0, 13, 8, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1];
        let mut data = Cursor::new(&v);
        // Test one shot
        match r.readable(&mut data) {
            RRes::Success(Message::Cancel {
                index,
                begin,
                length,
            }) => {
                assert_eq!(index, 1);
                assert_eq!(begin, 1);
                assert_eq!(length, 1);
            }
            _ => {
                unreachable!();
            }
        }
    }

    #[test]
    fn test_read_port() {
        let mut r = Reader::new();
        r.state = State::Len;
        let data = vec![0u8, 0, 0, 3, 9, 0x1A, 0xE1];
        test_message(data, Message::Port(6881));
    }

    #[test]
    fn test_read_handshake() {
        use crate::PEER_ID;
        let mut r = Reader::new();
        let m = Message::Handshake {
            rsv: [0; 8],
            hash: [0; 20],
            id: *PEER_ID,
        };
        let mut data = vec![0; 68];
        m.encode(&mut data[..]).unwrap();
        let mut c = Cursor::new(&data);
        assert_eq!(r.readable(&mut c).unwrap().unwrap(), m);
    }
}
