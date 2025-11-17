use std::fs::File;
use std::io;
use std::os::unix::fs::MetadataExt;

use rustix::io::Errno;

/// Returns `true` if `f` is sparse and `false` otherwise.
pub fn is_sparse(f: &File) -> io::Result<bool> {
    let stat = f.metadata()?;
    let f = rustix::io::dup(f)?;
    let pos = rustix::fs::seek(f, rustix::fs::SeekFrom::Hole(0))?;
    Ok(pos < stat.size())
}

/// Sets the length of `f` to `len`. On success, returns `Ok(is_sparsely_allocated)` if `f`'s
/// length was set to `len`, or an `io::Error` otherwise.
pub fn fallocate(f: &File, len: u64) -> io::Result<bool> {
    loop {
        match rustix::fs::fallocate(f, rustix::fs::FallocateFlags::empty(), 0, len) {
            Ok(_) => return Ok(true),
            Err(Errno::NOSYS) | Err(Errno::OPNOTSUPP) => {
                f.set_len(len)?;
                return Ok(false);
            }
            Err(Errno::INTR) => continue,
            Err(e) => return Err(io::Error::from_raw_os_error(e.raw_os_error())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;

    #[test]
    fn fallocate_and_is_sparse_match() {
        let test_file = tempfile::tempfile().unwrap();
        match fallocate(&test_file, 88_888_888) {
            Ok(is_sparsely_allocated) => {
                assert_matches!(is_sparse(&test_file), Ok(val) if val == is_sparsely_allocated);
                assert_eq!(test_file.metadata().unwrap().len(), 88_888_888);
            }
            // Ignore errors as some operating systems and/or filesystems do not support this
            // operation.
            // TODO: If the Rust test harness ever gets the option to mark tests as ignored/skipped
            // at runtime, use that functionality here.
            Err(_) => (),
        }
    }

    #[test]
    fn is_sparse_regular_file() {
        let mut test_file = tempfile::tempfile().unwrap();
        test_file.write_all(b"Hello world!").unwrap();
        assert_matches!(is_sparse(&test_file), Ok(false));
    }

    #[test]
    fn is_sparse_ftruncate() {
        let mut test_file = tempfile::tempfile().unwrap();
        rustix::fs::ftruncate(&test_file, 8).unwrap();
        assert_matches!(is_sparse(&test_file), Ok(true));

        // Now fill in the bytes...
        test_file.write_all(b"12345678").unwrap();
        assert_matches!(is_sparse(&test_file), Ok(false));
    }
}
