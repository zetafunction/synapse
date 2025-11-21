use std::sync::{Arc, atomic, mpsc};
use std::{io, process, thread};

use ip_network_table::IpNetworkTable;

use crate::config::Config;
use crate::control::acio;
use crate::{SHUTDOWN, THROT_TOKS};
use crate::{args, control, disk, log, rpc, throttle, tracker};

pub fn init(args: args::Args) -> Result<(), ()> {
    if let Some(level) = args.level {
        log::log_init(level);
    } else if cfg!(debug_assertions) {
        log::log_init(log::LogLevel::Debug);
    } else {
        log::log_init(log::LogLevel::Info);
    }

    info!("Initializing");

    if let Err(e) = init_signals() {
        error!("Failed to initialize signal handlers: {}", e);
        return Err(());
    }
    Ok(())
}

pub fn run(config: Arc<Config>) -> Result<(), ()> {
    match init_threads(config) {
        Ok(threads) => {
            for thread in threads {
                if thread.join().is_err() {
                    error!("Unclean shutdown detected, terminating");
                    return Err(());
                }
            }
            info!("Shutdown complete");
            Ok(())
        }
        Err(e) => {
            error!("Couldn't initialize synapse: {}", e);
            Err(())
        }
    }
}

fn init_threads(config: Arc<Config>) -> io::Result<Vec<thread::JoinHandle<()>>> {
    let cpoll = amy::Poller::new()?;
    let mut creg = cpoll.get_registrar();
    let (dh, disk_broadcast, dhj) = disk::start(config.clone(), &mut creg)?;
    let (rh, rhj) = rpc::RPC::start(config.clone(), &mut creg, disk_broadcast.clone())?;
    let (th, thj) = tracker::Tracker::start(config.clone(), &mut creg, disk_broadcast.clone())?;
    let chans = acio::ACChans {
        disk_tx: dh.tx,
        disk_rx: dh.rx,
        rpc_tx: rh.tx,
        rpc_rx: rh.rx,
        trk_tx: th.tx,
        trk_rx: th.rx,
    };
    let (tx, rx) = mpsc::channel();
    let cdb = disk_broadcast.clone();
    let chj = thread::Builder::new()
        .name("control".to_string())
        .spawn(move || {
            let throttler = throttle::Throttler::new(None, None, THROT_TOKS, &creg).unwrap();
            let acio = acio::ACIO::new(config.clone(), cpoll, creg, chans)
                .expect("Could not initialize IO");
            let ip_filter = {
                let mut table = IpNetworkTable::new();
                for (k, v) in config.ip_filter.iter() {
                    table.insert(*k, *v);
                    debug!("Add ip_filter entry: prefix={k}, weight={v}",);
                }
                table
            };
            match control::Control::new(config, ip_filter, acio, throttler, cdb) {
                Ok(mut c) => {
                    tx.send(Ok(())).unwrap();
                    c.run();
                }
                Err(e) => {
                    tx.send(Err(e)).unwrap();
                }
            }
        })
        .unwrap();
    rx.recv().unwrap()?;

    Ok(vec![chj, dhj, rhj, thj])
}

fn init_signals() -> Result<(), ctrlc::Error> {
    ctrlc::set_handler(move || {
        if SHUTDOWN.load(atomic::Ordering::SeqCst) {
            info!("Terminating process!");
            process::abort();
        } else {
            info!("Shutting down cleanly. Interrupt again to shut down immediately.");
            SHUTDOWN.store(true, atomic::Ordering::SeqCst);
        }
    })
}
