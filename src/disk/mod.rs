mod cache;
mod job;

pub use self::job::Ctx;
pub use self::job::Location;
pub use self::job::Request;
pub use self::job::Response;

use std::collections::VecDeque;
use std::sync::Arc;
use std::{fs, io, thread};

use self::cache::{BufCache, FileCache};
use self::job::JobRes;
use crate::config::Config;
use crate::worker;

#[cfg(test)]
mod tests;

const POLL_INT_MS: usize = 1000;
const JOB_TIME_SLICE: u64 = 150;

pub struct Disk {
    config: Arc<Config>,
    worker: worker::Worker<Request, Response>,
    jobs_rx: flume::Receiver<Request>,
    files: FileCache,
    active: VecDeque<Request>,
    sequential: VecDeque<Request>,
    bufs: BufCache,
}

impl Disk {
    pub fn new(
        config: Arc<Config>,
        worker: worker::Worker<Request, Response>,
        jobs_rx: flume::Receiver<Request>,
    ) -> Disk {
        Disk {
            worker,
            jobs_rx,
            files: FileCache::new(config.net.max_open_files),
            bufs: BufCache::new(),
            active: VecDeque::new(),
            sequential: VecDeque::new(),
            config,
        }
    }

    pub async fn run(&mut self) {
        let sd = &self.config.disk.session;
        fs::create_dir_all(sd).unwrap();

        loop {}
        /*

        while let Some(request) =  self.worker.rx.next() {
            match self.poll.wait(POLL_INT_MS) {
                Ok(_) => {
                    if self.handle_events() {
                        break;
                    }
                }
                Err(e) => {
                    error!("Failed to poll for events: {}", e);
                }
            }
            if !self.active.is_empty() && self.handle_active() {
                break;
            }
        }
        */

        // Try to finish up remaining jobs
        for job in self.active.drain(..) {
            if job.concurrent() {
                job.execute(&self.config.disk, &mut self.files, &mut self.bufs)
                    .ok();
            }
        }
    }

    fn enqueue_req(&mut self, req: Request) {
        if req.concurrent() || !self.active.iter().any(|r| !r.concurrent()) {
            self.active.push_back(req);
        } else {
            self.sequential.push_back(req);
        }
    }

    fn handle_active(&mut self) -> bool {
        let mut rotate = 1;
        while let Some(j) = self.active.pop_front() {
            let tid = j.tid();
            let seq = !j.concurrent();
            let mut done = false;
            match j.execute(&self.config.disk, &mut self.files, &mut self.bufs) {
                Ok(JobRes::Resp(r)) => {
                    done = true;
                    self.worker.tx.send(r).ok();
                }
                Ok(JobRes::Update(s, r)) => {
                    self.worker.tx.send(r).ok();
                    if rotate % 3 == 0 {
                        self.active.push_back(s);
                    } else {
                        self.active.push_front(s);
                    }
                }
                Ok(JobRes::Paused(s)) => {
                    if rotate % 3 == 0 {
                        self.active.push_back(s);
                    } else {
                        self.active.push_front(s);
                    }
                }
                Ok(JobRes::Done) => {
                    done = true;
                }
                Err(e) => {
                    done = true;
                    if let Some(t) = tid {
                        self.worker.tx.send(Response::error(t, e)).ok();
                    } else {
                        error!("Disk job failed: {}", e);
                    }
                }
            }
            if done
                && seq
                && let Some(r) = self.sequential.pop_front()
            {
                self.active.push_back(r);
            }
            match self.poll.wait(0) {
                Ok(_) => {
                    if self.handle_events() {
                        return true;
                    }
                }
                Err(e) => {
                    error!("Failed to poll for events: {:?}", e);
                }
            }
            rotate += 1;
        }
        false
    }

    pub fn handle_events(&mut self) -> bool {
        loop {
            match self.ch.recv() {
                Ok(Request::Shutdown) => {
                    return true;
                }
                Ok(mut r) => {
                    let tid = r.tid();
                    if let Err(e) = r.setup()
                        && let Some(t) = tid
                    {
                        self.tx.send(Response::error(t, e)).ok();
                    }
                    self.enqueue_req(r);
                }
                _ => break,
            }
        }
        while let Ok(mut r) = self.jobs.try_recv() {
            if r.setup().is_err() {
                continue;
            }
            self.enqueue_req(r);
        }
        false
    }
}

pub fn start(
    config: Arc<Config>,
    creg: &mut amy::Registrar,
) -> io::Result<(
    worker::WorkerHandle<Request, Response>,
    flume::Sender<Request>,
    thread::JoinHandle<()>,
)> {
    let (worker_handle, worker) = worker::Worker::new(creg)?;
    let (jobs_tx, jobs_rx) = flume::unbounded::<Request>();
    let h = worker.run("disk", async move |worker| {
        Disk::new(config, worker, jobs_rx).run().await
    })?;
    Ok((worker_handle, jobs_tx, h))
}
