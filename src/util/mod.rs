pub mod http;
mod io;
pub mod native;

use std::collections::{HashMap, HashSet};
use std::fmt::Write as FWrite;
use std::hash::BuildHasherDefault;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;

use byteorder::{BigEndian, ByteOrder};
use metrohash::MetroHash;
use rand::distr::{Alphanumeric, SampleString};
use rand::{self, Rng};
use sha1::{Digest, Sha1};
use url::Url;

pub type FHashMap<K, V> = fnv::FnvHashMap<K, V>;
pub type FHashSet<T> = fnv::FnvHashSet<T>;
pub type UHashMap<T> = FHashMap<usize, T>;
pub type UHashSet = FHashSet<usize>;

pub type MBuildHasher = BuildHasherDefault<MetroHash>;
pub type MHashMap<K, V> = HashMap<K, V, MBuildHasher>;
pub type MHashSet<T> = HashSet<T, MBuildHasher>;
pub type SHashMap<T> = MHashMap<String, T>;

pub use self::io::{aread, awrite, io_err, io_err_val, IOR};

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub struct UnlimitedOrU64(Option<std::num::NonZeroU64>);

impl UnlimitedOrU64 {
    pub fn new(val: u64) -> UnlimitedOrU64 {
        UnlimitedOrU64(std::num::NonZeroU64::new(val))
    }
}

impl PartialEq<usize> for UnlimitedOrU64 {
    fn eq(&self, other: &usize) -> bool {
        self.0.is_some_and(|val| val.get() == *other as u64)
    }
}

impl PartialEq<UnlimitedOrU64> for usize {
    fn eq(&self, other: &UnlimitedOrU64) -> bool {
        other == self
    }
}

impl PartialOrd<usize> for UnlimitedOrU64 {
    fn partial_cmp(&self, other: &usize) -> Option<std::cmp::Ordering> {
        match self.0 {
            None => Some(std::cmp::Ordering::Greater),
            Some(val) => val.get().partial_cmp(&(*other as u64)),
        }
    }
}

impl PartialOrd<UnlimitedOrU64> for usize {
    fn partial_cmp(&self, other: &UnlimitedOrU64) -> Option<std::cmp::Ordering> {
        other.partial_cmp(self).map(std::cmp::Ordering::reverse)
    }
}

impl serde::Serialize for UnlimitedOrU64 {
    fn serialize<S>(&self, serializer: S) -> core::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u64(self.0.map_or(0, |x| x.into()))
    }
}

impl<'de> serde::Deserialize<'de> for UnlimitedOrU64 {
    fn deserialize<D>(deserializer: D) -> core::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let val: u64 = serde::Deserialize::deserialize(deserializer)?;
        Ok(UnlimitedOrU64::new(val))
    }
}

pub fn random_sample<A, T>(iter: A) -> Option<T>
where
    A: Iterator<Item = T>,
{
    let mut elem = None;
    let mut i = 1f64;
    let mut rng = rand::rng();
    for new_item in iter {
        if rng.random::<f64>() < (1f64 / i) {
            elem = Some(new_item);
        }
        i += 1.0;
    }
    elem
}

pub fn random_string(len: usize) -> String {
    Alphanumeric.sample_string(&mut rand::rng(), len)
}

pub fn sha1_hash(data: &[u8]) -> [u8; 20] {
    let mut ctx = Sha1::new();
    ctx.update(data);
    ctx.finalize().into()
}

pub fn peer_rpc_id(torrent: &[u8; 20], peer: u64) -> String {
    const PEER_ID: &[u8] = b"PEER";

    let mut ctx = Sha1::new();
    ctx.update(torrent);
    ctx.update(PEER_ID);
    ctx.update(&peer.to_be_bytes());
    hash_to_id(&ctx.finalize())
}

pub fn file_rpc_id(torrent: &[u8; 20], file: &Path) -> String {
    const FILE_ID: &[u8] = b"FILE";
    let mut ctx = Sha1::new();
    ctx.update(torrent);
    ctx.update(FILE_ID);
    ctx.update(file.as_os_str().as_encoded_bytes());
    hash_to_id(&ctx.finalize())
}

pub fn trk_rpc_id(torrent: &[u8; 20], url: &url::Url) -> String {
    const TRK_ID: &[u8] = b"TRK";
    let mut ctx = Sha1::new();
    ctx.update(torrent);
    ctx.update(TRK_ID);
    ctx.update(url.as_str().as_bytes());
    hash_to_id(&ctx.finalize())
}

pub fn hash_to_id(hash: &[u8]) -> String {
    let mut hash_str = String::with_capacity(hash.len() * 2);
    for i in hash {
        write!(&mut hash_str, "{i:02X}").unwrap();
    }
    hash_str
}

pub fn id_to_hash(s: &str) -> Option<[u8; 20]> {
    let mut data = [0u8; 20];
    if s.len() != 40 {
        return None;
    }
    let mut c = s.chars();
    for i in &mut data {
        if let (Some(a), Some(b)) = (hex_to_bit(c.next().unwrap()), hex_to_bit(c.next().unwrap())) {
            *i = a << 4 | b
        } else {
            return None;
        }
    }
    Some(data)
}

fn hex_to_bit(c: char) -> Option<u8> {
    let r = match c {
        '0' => 0,
        '1' => 1,
        '2' => 2,
        '3' => 3,
        '4' => 4,
        '5' => 5,
        '6' => 6,
        '7' => 7,
        '8' => 8,
        '9' => 9,
        'a' | 'A' => 10,
        'b' | 'B' => 11,
        'c' | 'C' => 12,
        'd' | 'D' => 13,
        'e' | 'E' => 14,
        'f' | 'F' => 15,
        _ => return None,
    };
    Some(r)
}

pub fn bytes_to_addr(p: &[u8]) -> SocketAddr {
    let ip = Ipv4Addr::new(p[0], p[1], p[2], p[3]);
    SocketAddr::V4(SocketAddrV4::new(ip, BigEndian::read_u16(&p[4..])))
}

pub fn addr_to_bytes(addr: &SocketAddr) -> [u8; 6] {
    let mut data = [0u8; 6];
    match *addr {
        SocketAddr::V4(s) => {
            let oct = s.ip().octets();
            data[0] = oct[0];
            data[1] = oct[1];
            data[2] = oct[2];
            data[3] = oct[3];
            BigEndian::write_u16(&mut data[4..], s.port());
        }
        _ => panic!("IPv6 DHT not supported"),
    }
    data
}

pub fn find_subseq(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_unlimitedoru64_partial_eq() {
        const EIGHT: UnlimitedOrU64 = UnlimitedOrU64(std::num::NonZeroU64::new(8));
        assert!(EIGHT == 8);
        assert!(8 == EIGHT);
        assert!(EIGHT != 9);
        assert!(9 != EIGHT);

        const UNLIMITED: UnlimitedOrU64 = UnlimitedOrU64(None);
        assert!(UNLIMITED != 8);
        assert!(8 != UNLIMITED);
        assert!(UNLIMITED != 9);
        assert!(9 != UNLIMITED);
    }

    #[test]
    fn test_unlimitedoru64_partial_ord() {
        const EIGHT: UnlimitedOrU64 = UnlimitedOrU64(std::num::NonZeroU64::new(8));
        assert!(EIGHT > 7);
        assert!(7 < EIGHT);
        assert!(EIGHT < 9);
        assert!(9 > EIGHT);

        const UNLIMITED: UnlimitedOrU64 = UnlimitedOrU64(None);
        assert!(UNLIMITED > 8);
        assert!(8 < UNLIMITED);
        assert!(UNLIMITED > 9);
        assert!(9 < UNLIMITED);
    }

    #[test]
    fn test_hash_enc() {
        let hash = [8u8; 20];
        let s = hash_to_id(&hash);
        assert_eq!(id_to_hash(&s).unwrap(), hash);
    }
}
