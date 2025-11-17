use std::ffi::OsString;
use std::{fs, io, mem, path};

use std::io::{Read, Seek, SeekFrom, Write};

use crate::util::{native, MHashMap};

const PB_LEN: usize = 256;

/// A simple allocation pool to reduce allocations. Currently hardcoded to hold two `PathBuf`s and
/// one `Vec<u8>`. Use `data()` to borrow these objects; they will automatically be returned to the
/// pool at the end of the scope.
pub struct BufCache {
    path_a: OsString,
    path_b: OsString,
    buf: Vec<u8>,
}

pub struct FileCache {
    files: MHashMap<path::PathBuf, Entry>,
    max_size: usize,
}

pub enum RequestedSize {
    WithoutFallocate(u64),
    WithFallocate(u64),
}

enum Mode {
    ReadOnly,
    ReadWrite(RequestedSize),
}

#[derive(Debug)]
enum State {
    ReadOnly,
    ReadWrite { alloc_failed: bool, sparse: bool },
}

#[derive(Debug)]
pub struct Entry {
    used: bool,
    state: State,
    file: fs::File,
}

pub struct TempPB<'a> {
    path: path::PathBuf,
    buf: &'a mut OsString,
}

pub struct TempBuf<'a> {
    buf: &'a mut Vec<u8>,
}

impl TempBuf<'_> {
    pub fn get(&mut self, len: usize) -> &mut [u8] {
        self.buf.reserve(len);
        if self.buf.len() < len {
            self.buf.resize(len, 0u8);
        }
        &mut self.buf[..len]
    }
}

fn get_pb(buf: &mut OsString) -> TempPB<'_> {
    debug_assert!(buf.capacity() >= PB_LEN);
    let path = mem::replace(buf, OsString::with_capacity(0)).into();
    TempPB { buf, path }
}

impl TempPB<'_> {
    pub fn get<P: AsRef<path::Path>>(&mut self, base: P) -> &mut path::PathBuf {
        self.clear();
        self.path.push(base.as_ref());
        &mut self.path
    }

    fn clear(&mut self) {
        let mut s =
            mem::replace(&mut self.path, OsString::with_capacity(0).into()).into_os_string();
        s.clear();
        self.path = s.into();
    }
}

impl Drop for TempPB<'_> {
    fn drop(&mut self) {
        let mut path =
            mem::replace(&mut self.path, OsString::with_capacity(0).into()).into_os_string();
        mem::swap(self.buf, &mut path);
        self.buf.clear();
    }
}

impl BufCache {
    pub fn new() -> BufCache {
        BufCache {
            path_a: OsString::with_capacity(PB_LEN),
            path_b: OsString::with_capacity(PB_LEN),
            buf: Vec::with_capacity(1_048_576),
        }
    }

    pub fn data(&mut self) -> (TempBuf<'_>, TempPB<'_>, TempPB<'_>) {
        (
            TempBuf { buf: &mut self.buf },
            get_pb(&mut self.path_a),
            get_pb(&mut self.path_b),
        )
    }
}

impl FileCache {
    pub fn new(max_size: usize) -> FileCache {
        FileCache {
            files: MHashMap::default(),
            max_size,
        }
    }

    pub fn read_file_range(
        &mut self,
        path: &path::Path,
        offset: u64,
        buf: &mut [u8],
    ) -> io::Result<()> {
        self.ensure_exists(path, Mode::ReadOnly)?;
        let entry = self
            .files
            .get_mut(path)
            .ok_or(io::Error::from(io::ErrorKind::NotFound))?;
        entry.file.seek(SeekFrom::Start(offset))?;
        entry.file.read_exact(buf)?;
        Ok(())
    }

    pub fn write_file_range(
        &mut self,
        path: &path::Path,
        size: RequestedSize,
        offset: u64,
        buf: &[u8],
    ) -> io::Result<()> {
        self.ensure_exists(path, Mode::ReadWrite(size))?;
        let entry = self.files.get_mut(path).unwrap();
        entry.file.seek(SeekFrom::Start(offset))?;
        entry.file.write_all(buf)?;
        Ok(())
    }

    pub fn remove_file(&mut self, path: &path::Path) {
        self.files.remove(path);
    }

    pub fn retain<F: Fn(&path::Path) -> bool>(&mut self, f: F) {
        self.files.retain(|k, _| f(k));
    }

    pub fn flush_file(&mut self, path: &path::Path) {
        self.files.get_mut(path).map(|e| e.file.sync_all().ok());
    }

    // TODO: Return a ref to the entry to save some lookups
    fn ensure_exists(&mut self, path: &path::Path, mode: Mode) -> io::Result<()> {
        if let Some(entry) = self.files.get_mut(path) {
            match &mode {
                Mode::ReadOnly => return Ok(()),
                Mode::ReadWrite(requested_size) => match &mut entry.state {
                    State::ReadOnly => {
                        // Evict the entry, since the opened file isn't writable and fall through
                        // to create a new entry below.
                        self.files.remove(path);
                    }
                    State::ReadWrite {
                        alloc_failed,
                        sparse,
                    } => {
                        if let RequestedSize::WithFallocate(size) = requested_size
                            && *sparse
                            && !*alloc_failed
                        {
                            let file = fs::OpenOptions::new().write(true).read(true).open(path)?;
                            *alloc_failed = !native::fallocate(&file, *size)?;
                            if !*alloc_failed {
                                *sparse = false;
                            }
                        }
                        return Ok(());
                    }
                },
            }
        }

        if self.files.len() >= self.max_size {
            // TODO: While it's unlikely, it seems possible that this might end up removing nothing
            // from the cache. Perhaps eventual consistency here is OK?
            let mut removal = None;
            // We rely on random iteration order to prove us something close to a "clock hand"
            // like algorithm
            for (id, entry) in &mut self.files {
                if entry.used {
                    entry.used = false;
                } else {
                    removal = Some(id.clone());
                }
            }
            if let Some(f) = removal {
                self.remove_file(&f);
            }
        }

        self.files.insert(
            path.to_path_buf(),
            match mode {
                Mode::ReadOnly => {
                    let file = fs::OpenOptions::new().read(true).open(path)?;

                    Entry {
                        file,
                        used: true,
                        state: State::ReadOnly,
                    }
                }
                Mode::ReadWrite(requested_size) => {
                    fs::create_dir_all(path.parent().unwrap())?;
                    let file = fs::OpenOptions::new()
                        .create(true)
                        .truncate(false)
                        .read(true)
                        .write(true)
                        .open(path)?;

                    let alloc_failed = match requested_size {
                        RequestedSize::WithFallocate(size) => {
                            if file.metadata()?.len() != size {
                                let res = !native::fallocate(&file, size)?;
                                debug!("Attempted to fallocate {:?}: success {}!", path, !res);
                                res
                            } else {
                                false
                            }
                        }
                        RequestedSize::WithoutFallocate(size) => {
                            file.set_len(size)?;
                            false
                        }
                    };

                    let sparse = native::is_sparse(&file)?;

                    Entry {
                        file,
                        used: true,
                        state: State::ReadWrite {
                            alloc_failed,
                            sparse,
                        },
                    }
                }
            },
        );

        Ok(())
    }
}

impl Drop for FileCache {
    fn drop(&mut self) {
        for (_, entry) in self.files.drain() {
            entry.file.sync_all().ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tempbuf() {
        let mut data = vec![];
        let mut buf = TempBuf { buf: &mut data };
        assert_eq!(buf.get(10).len(), 10);
        assert_eq!(buf.get(20).len(), 20);
        assert_eq!(buf.get(10).len(), 10);
        assert_eq!(buf.get(30).len(), 30);
        assert_eq!(buf.get(10).len(), 10);
    }

    // TODO: Add tests for eviction?
    // TODO: Add tests with and without fallocate?
    // TODO: Add tests for delayed fallocate?
    #[test]
    fn test_read_file_range_with_nonexistent_file() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let mut cache = FileCache::new(8);

        // If the file does not exist, `read_file_range()` should not create it and no cache entry
        // should be created.
        let path = tmp_dir.path().join("nonexistent");
        let mut buffer = [0; 8];
        assert_matches!(cache.read_file_range(&path, 0, &mut buffer), Err(_));
        assert_matches!(fs::exists(&path), Ok(false));
        assert!(!cache.files.contains_key(&path));

        // Parent directories should not be created either.
        let parent_path = tmp_dir.path().join("parentdir");
        let path = parent_path.join("nonexistent");
        let mut buffer = [0; 8];
        assert_matches!(cache.read_file_range(&path, 0, &mut buffer), Err(_));
        assert_matches!(fs::exists(&parent_path), Ok(false));
        assert_matches!(fs::exists(&path), Ok(false));
        assert!(!cache.files.contains_key(&path));
    }

    #[test]
    fn test_write_file_range_with_nonexistent_file() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let mut cache = FileCache::new(8);
        let hello_world = "Hello world!";

        // In contrast, `write_file_range()` should create the file if it doesn't exist.
        let path = tmp_dir.path().join("file");
        assert_matches!(
            cache.write_file_range(
                &path,
                RequestedSize::WithFallocate(100),
                0,
                hello_world.as_bytes()
            ),
            Ok(())
        );
        let contents = fs::read(&path).unwrap();
        let remainder = contents.strip_prefix(hello_world.as_bytes());
        assert!(remainder.is_some());
        // The rest of the buffer should be zeroed out.
        assert!(remainder.unwrap().iter().all(|&b| b == 0));
        assert_matches!(
            cache.files.get(&path),
            Some(Entry {
                used: _,
                state: State::ReadWrite { .. },
                file: _
            })
        );

        // It should also create parent directories as needed.
        let path = tmp_dir.path().join("nested/parent/file");
        assert_matches!(
            cache.write_file_range(
                &path,
                RequestedSize::WithFallocate(hello_world.as_bytes().len() as u64),
                0,
                hello_world.as_bytes()
            ),
            Ok(())
        );
        let contents = fs::read(&path).unwrap();
        assert_eq!(contents, hello_world.as_bytes());
        assert_matches!(
            cache.files.get(&path),
            Some(Entry {
                used: _,
                state: State::ReadWrite { .. },
                file: _
            })
        );
    }

    #[test]
    fn test_read_file_range_with_existing_file() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let mut cache = FileCache::new(8);

        let path = tmp_dir.path().join("file");
        assert!(fs::write(&path, b"Hello world!").is_ok());

        let mut buffer = [0; 6];
        assert_matches!(cache.read_file_range(&path, 0, &mut buffer), Ok(()));
        assert_eq!(&buffer, b"Hello ");
        assert_matches!(
            cache.files.get(&path),
            Some(Entry {
                used: _,
                state: State::ReadOnly,
                file: _
            })
        );

        assert_matches!(cache.read_file_range(&path, 6, &mut buffer), Ok(()));
        assert_eq!(&buffer, b"world!");
        assert_matches!(
            cache.files.get(&path),
            Some(Entry {
                used: _,
                state: State::ReadOnly,
                file: _
            })
        );
    }

    #[test]
    fn test_write_file_range_with_existing_file() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let mut cache = FileCache::new(8);

        let path = tmp_dir.path().join("file");
        assert_matches!(
            cache.write_file_range(&path, RequestedSize::WithFallocate(12), 0, b"Hello "),
            Ok(())
        );
        assert_matches!(
            cache.files.get(&path),
            Some(Entry {
                used: _,
                state: State::ReadWrite { .. },
                file: _
            })
        );

        assert_matches!(
            cache.write_file_range(&path, RequestedSize::WithFallocate(12), 6, b"world!"),
            Ok(())
        );
        assert_matches!(
            cache.files.get(&path),
            Some(Entry {
                used: _,
                state: State::ReadWrite { .. },
                file: _
            })
        );

        assert_eq!(&fs::read(&path).unwrap(), b"Hello world!");
    }

    #[test]
    fn test_read_file_range_then_write_file_range_on_existing_file() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let mut cache = FileCache::new(8);

        let path = tmp_dir.path().join("file");
        assert!(fs::write(&path, b"Hel------ld!").is_ok());

        let mut buffer = [0; 12];
        assert_matches!(cache.read_file_range(&path, 0, &mut buffer), Ok(()));
        assert_eq!(&buffer, b"Hel------ld!");
        assert_matches!(
            cache.files.get(&path),
            Some(Entry {
                used: _,
                state: State::ReadOnly,
                file: _
            })
        );

        assert_matches!(
            cache.write_file_range(&path, RequestedSize::WithFallocate(12), 3, b"lo wor"),
            Ok(())
        );
        // Cache entry should be updated since the previous cache entry was incompatible.
        assert_matches!(
            cache.files.get(&path),
            Some(Entry {
                used: _,
                state: State::ReadWrite { .. },
                file: _
            })
        );
        assert_eq!(&fs::read(&path).unwrap(), b"Hello world!");
    }

    #[test]
    fn test_write_file_range_then_read_file_range_on_existing_file() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let mut cache = FileCache::new(8);

        let path = tmp_dir.path().join("file");
        assert!(fs::write(&path, b"Hel------ld!").is_ok());
        assert_matches!(
            cache.write_file_range(&path, RequestedSize::WithFallocate(12), 3, b"lo wor"),
            Ok(())
        );
        assert_matches!(
            cache.files.get(&path),
            Some(Entry {
                used: _,
                state: State::ReadWrite { .. },
                file: _
            })
        );

        let mut buffer = [0; 12];
        assert_matches!(cache.read_file_range(&path, 0, &mut buffer), Ok(()));
        assert_eq!(&buffer, b"Hello world!");
        // The read-write cache entry should still be present.
        assert_matches!(
            cache.files.get(&path),
            Some(Entry {
                used: _,
                state: State::ReadWrite { .. },
                file: _
            })
        );
    }
}
