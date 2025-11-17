#[cfg(feature = "mimalloc")]
use mimalloc::MiMalloc;
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate serde_derive;

#[cfg(test)]
#[macro_use]
extern crate assert_matches;

use synapse_bencode as bencode;
use synapse_protocol as protocol;
use synapse_rpc as rpc_lib;
use synapse_session as session;

#[macro_use]
mod log;
#[macro_use]
mod util;
mod args;
mod buffers;
mod config;
mod control;
mod disk;
mod handle;
mod init;
mod rpc;
mod socket;
mod stat;
mod throttle;
mod torrent;
mod tracker;
mod worker;

use ip_network_table::IpNetworkTable;
use rand::seq::IndexedRandom;
use std::process;
use std::sync::atomic;

pub use crate::protocol::DHT_EXT;
pub use crate::protocol::EXT_PROTO;
pub use crate::protocol::UT_META_ID;
pub use crate::protocol::UT_PEX_ID;

/// Throttler max token amount
pub const THROT_TOKS: usize = 2 * 1024 * 1024;

pub static SHUTDOWN: atomic::AtomicBool = atomic::AtomicBool::new(false);

lazy_static! {
    pub static ref CONFIG: config::Config = config::Config::load();
    pub static ref PEER_ID: [u8; 20] = {
        let mut pid = [0u8; 20];
        let prefix = b"-SY0010-";
        pid[..prefix.len()].copy_from_slice(&prefix[..]);

        // Based on libtorrent's list of URL-safe characters.
        const URL_SAFE_CHARACTERS: &[u8] =
            "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-_.!~*()".as_bytes();

        let mut rng = rand::rng();
        for p in pid.iter_mut().skip(prefix.len()) {
            *p = *URL_SAFE_CHARACTERS.choose(&mut rng).unwrap();
        }
        pid
    };
    pub static ref DL_TOKEN: String = util::random_string(20);
    pub static ref IP_FILTER: IpNetworkTable<u8> = {
        let mut table = IpNetworkTable::new();

        for k in CONFIG.ip_filter.keys() {
            table.insert(*k, CONFIG.ip_filter[k]);
            debug!(
                "Add ip_filter entry: prefix={}, weight={}",
                k, CONFIG.ip_filter[k]
            );
        }
        table
    };
}

fn main() {
    let args = args::args();
    match init::init(args) {
        Ok(()) => {}
        Err(()) => {
            error!("Failed to initialize synapse!");
            process::exit(1);
        }
    }
    info!("Initialized, starting!");
    match init::run() {
        Ok(()) => process::exit(0),
        Err(()) => process::exit(1),
    }
}
