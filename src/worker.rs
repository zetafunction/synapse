use std::fmt::Debug;
use std::{io, thread};

pub struct WorkerHandle<I, O> {
    pub tx: futures::channel::mpsc::UnboundedSender<I>,
    pub rx: amy::Receiver<O>,
}

pub struct Worker<I, O> {
    pub rx: futures::channel::mpsc::UnboundedReceiver<I>,
    pub tx: amy::Sender<O>,
}

impl<I: Debug + Send + 'static, O: Debug + Send + 'static> Worker<I, O> {
    /// `reg` is the controller's registrar.
    pub fn new(reg: &mut amy::Registrar) -> io::Result<(WorkerHandle<I, O>, Self)> {
        let (ctx, crx) = reg.channel::<O>()?;
        let (wtx, wrx) = futures::channel::mpsc::unbounded::<I>();
        let worker_handle = WorkerHandle { tx: wtx, rx: crx };
        let worker = Self { tx: ctx, rx: wrx };
        Ok((worker_handle, worker))
    }

    pub fn run<F: AsyncFnOnce(Self) + Send + 'static>(
        self,
        thread_name: &'static str,
        f: F,
    ) -> io::Result<thread::JoinHandle<()>> {
        let builder = thread::Builder::new().name(thread_name.to_owned());
        builder.spawn(move || {
            debug!("{} worker started", thread_name);
            let runtime = compio::runtime::Runtime::new().unwrap();
            runtime.block_on(f(self));
            debug!("{} worker completed", thread_name);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[derive(Debug)]
    struct Ping {}
    #[derive(Debug)]
    struct Pong {}

    #[test]
    fn ping_pong_worker() {
        let mut poller = amy::Poller::new().unwrap();
        let mut reg = poller.get_registrar();
        let (worker_handle, worker) = Worker::<Ping, Pong>::new(&mut reg).unwrap();
        worker_handle.tx.unbounded_send(Ping {}).unwrap();
        let join_handle = worker
            .run("worker", async |mut worker| {
                let Some(_ping) = worker.rx.next().await else {
                    panic!("failed to receive message");
                };
                assert_matches!(worker.tx.send(Pong {}), Ok(()));
            })
            .unwrap();
        let _ = poller.wait(10 * 1000).unwrap();
        assert_matches!(worker_handle.rx.try_recv(), Ok(Pong {}));
        join_handle.join().unwrap();
    }

    #[test]
    fn pong_ping_worker() {
        let mut poller = amy::Poller::new().unwrap();
        let mut reg = poller.get_registrar();
        let (worker_handle, worker) = Worker::<Ping, Pong>::new(&mut reg).unwrap();
        let join_handle = worker
            .run("worker", async |mut worker| {
                assert_matches!(worker.tx.send(Pong {}), Ok(()));
                let Some(_ping) = worker.rx.next().await else {
                    panic!("failed to receive message");
                };
            })
            .unwrap();
        let _ = poller.wait(10 * 1000).unwrap();
        assert_matches!(worker_handle.rx.try_recv(), Ok(Pong {}));
        worker_handle.tx.unbounded_send(Ping {}).unwrap();
        join_handle.join().unwrap();
    }
}
