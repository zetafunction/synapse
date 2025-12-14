use std::collections::HashSet;

use super::*;
use crate::buffers::{BUF_SIZE, Buffer};
use crate::torrent::Info;
use crate::torrent::info::File;
use crate::{config, handle};

struct Env {
    session_dir: tempfile::TempDir,
    data_dir: tempfile::TempDir,
    poll: amy::Poller,
    reg: amy::Registrar,
    handle: handle::Handle<Response, Request>,
    jobs: amy::Sender<Request>,
    join_handle: Option<std::thread::JoinHandle<()>>,
}

impl Env {
    fn new() -> Self {
        let session_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let config = Arc::new(config::Config {
            disk: config::DiskConfig {
                session: session_dir.path().to_str().unwrap().to_string(),
                directory: data_dir.path().to_str().unwrap().to_string(),
                ..Default::default()
            },
            ..Default::default()
        });
        let poll = amy::Poller::new().unwrap();
        let mut reg = poll.get_registrar();
        let (handle, jobs, join_handle) = start(config, &mut reg).unwrap();
        Self {
            session_dir,
            data_dir,
            poll,
            reg,
            handle,
            jobs,
            join_handle: Some(join_handle),
        }
    }

    fn join(mut self) {
        assert_eq!(self.handle.send(Request::shutdown()), Ok(()));
        assert_matches!(self.join_handle.take().unwrap().join(), Ok(()));
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        assert!(self.join_handle.is_none());
    }
}

// TODO: Add this helper to the Info impl?
fn make_test_info(name: &str, files: &[File], piece_len: u64) -> Info {
    let total_len = files.iter().map(|f| f.length).sum();
    let piece_count = (total_len + piece_len - 1) / piece_len;
    let piece_count = usize::try_from(piece_count).unwrap();
    let piece_idx = Info::generate_piece_idx(piece_count, piece_len, &files);
    Info {
        name: name.to_string(),
        comment: None,
        creator: None,
        announce: None,
        piece_len: piece_len.try_into().unwrap(),
        total_len,
        hashes: vec![vec![0u8]; piece_count],
        hash: [0u8; 20],
        files: files.to_vec(),
        private: false,
        be_name: None,
        piece_idx,
        url_list: vec![],
    }
}

fn get_contexts_for_info(info: &Info) -> HashSet<Ctx> {
    (0..info.total_len)
        .step_by(info.piece_len.try_into().unwrap())
        .flat_map(|offset| {
            let piece_idx = (offset / u64::from(info.piece_len)).try_into().unwrap();
            let piece_len: u32 = std::cmp::min(info.piece_len.into(), info.total_len - offset)
                .try_into()
                .unwrap();
            (0..piece_len)
                .step_by(BUF_SIZE)
                .inspect(|begin| println!("{begin}"))
                .map(move |offset_in_piece| {
                    Ctx::new(
                        0,
                        0,
                        piece_idx,
                        offset_in_piece,
                        info.block_len(piece_idx, offset_in_piece),
                    )
                })
        })
        .collect()
}

#[test]
fn read() {
    let mut env = Env::new();
    let expected_data = b"012345678".repeat(11_111);
    let path = env.data_dir.path().join("abc");
    std::fs::write(&path, &expected_data).unwrap();

    let mut run_read_test = |piece_len| {
        let files = &[File {
            path: path.clone(),
            length: expected_data.len().try_into().unwrap(),
        }];
        let info = Arc::new(make_test_info("Test", files, piece_len));
        let mut pending_contexts = get_contexts_for_info(&info);
        let piece_len: usize = piece_len.try_into().unwrap();
        for context in &pending_contexts {
            let locs = Info::block_disk_locs(&info, context.idx, context.begin);
            env.jobs
                .send(Request::read(
                    context.clone(),
                    Buffer::get().unwrap(),
                    locs,
                    None,
                ))
                .unwrap();
        }
        while !pending_contexts.is_empty() {
            env.poll.wait(1000).unwrap();
            match env.handle.rx.try_recv() {
                Ok(Response::Read { context, data }) => {
                    assert!(pending_contexts.remove(&context));
                    let idx: usize = context.idx.try_into().unwrap();
                    let begin: usize = context.begin.try_into().unwrap();
                    let length: usize = context.length.try_into().unwrap();
                    assert_eq!(
                        expected_data[idx * piece_len + begin..idx * piece_len + begin + length],
                        data[0..length],
                    );
                }
                _ => panic!(),
            }
        }
    };

    run_read_test(16_384);
    run_read_test(32_768);
    run_read_test(65_536);
    run_read_test(131_072);

    env.join();
}

#[test]
fn write() {
    let mut env = Env::new();
    let expected_data = b"012345678".repeat(11_111);

    let mut run_write_test = |piece_len| {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("abc");
        let files = &[File {
            path: path.clone(),
            length: expected_data.len().try_into().unwrap(),
        }];
        let info = Arc::new(make_test_info("Test", files, piece_len));
        let mut pending_contexts = get_contexts_for_info(&info);
        let piece_len: usize = piece_len.try_into().unwrap();
        for context in &pending_contexts {
            let locs = Info::block_disk_locs(&info, context.idx, context.begin);
            let idx: usize = context.idx.try_into().unwrap();
            let begin: usize = context.begin.try_into().unwrap();
            let length: usize = context.length.try_into().unwrap();
            let mut buffer = Buffer::get().unwrap();
            buffer[0..length].copy_from_slice(
                &expected_data[idx * piece_len + begin..idx * piece_len + begin + length],
            );
            env.jobs
                .send(Request::write(
                    context.clone(),
                    buffer,
                    locs,
                    Some(tempdir.path().to_str().unwrap().to_owned()),
                ))
                .unwrap();
        }
        while !pending_contexts.is_empty() {
            env.poll.wait(1000).unwrap();
            match env.handle.rx.try_recv() {
                Ok(Response::Write { context }) => {
                    assert!(pending_contexts.remove(&context));
                }
                _ => panic!(),
            }
        }
        assert_eq!(expected_data, std::fs::read(&path).unwrap());
    };

    run_write_test(16_384);
    run_write_test(32_768);
    run_write_test(65_536);
    run_write_test(131_072);

    env.join();
}
