//! # Extension methods for openat::Dir and std::fs::File
//!
//! ```
//! use openat_ext::OpenatDirExt;
//! ```
//!
//! The `openat` crate is a low-level API, generally just exposing
//! thin wrappers for the underlying system call.  This crate offers
//! a number of common higher level convenience functions.
//!
//! More recently, there is also an `FileExt` available; it currently
//! just contains an optimized file copy method that will hopefully
//! go into the standard library.

#![deny(unused_results)]
#![deny(missing_docs)]
// We're just a wrapper around openat, shouldn't have any unsafe here.
#![forbid(unsafe_code)]

use libc;
use nix;
use openat;
use rand::Rng;
use std::ffi::OsStr;
use std::fs::File;
use std::io::prelude::*;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::prelude::FileExt as UnixFileExt;
use std::path::{Path, PathBuf};
use std::{fs, io};

/// Helper functions for openat::Dir
pub trait OpenatDirExt {
    /// Checking for nonexistent files (`ENOENT`) is by far the most common case of inspecting error
    /// codes in Unix.  Rust has a nice `Option<>` type, so this helper makes use of it and returns `Ok(None)`
    /// for nonexistent files.  All other errors are `Err`, and extant files are `Ok(Some<file>))`
    /// of course.
    fn open_file_optional<P: openat::AsPath>(&self, p: P) -> io::Result<Option<fs::File>>;

    /// Remove a file from the given directory.
    fn remove_file_optional<P: openat::AsPath>(&self, p: P) -> io::Result<()>;

    /// Like `open_file_optional()` except opens a directory via `openat::dir::sub_dir`.
    fn sub_dir_optional<P: openat::AsPath>(&self, p: P) -> io::Result<Option<openat::Dir>>;

    /// Like `metadata()` except returns `Ok(None)` for nonexistent paths.
    fn metadata_optional<P: openat::AsPath>(&self, p: P) -> io::Result<Option<openat::Metadata>>;

    /// On modern filesystems the directory entry contains the type; if available,
    /// return it.  Otherwise invoke `stat()`.
    fn get_file_type(&self, e: &openat::Entry) -> io::Result<openat::SimpleType>;

    /// Returns true iff file exists (may be a directory or symlink).  Symbolic links
    /// are not followed.
    fn exists<P: openat::AsPath>(&self, p: P) -> io::Result<bool>;

    /// Create a directory but don't error if it already exists.
    fn ensure_dir<P: openat::AsPath>(&self, p: P, mode: libc::mode_t) -> io::Result<()>;

    /// Create directory and all parents as necessary; no error is returned if directory already exists.
    fn ensure_dir_all<P: openat::AsPath>(&self, p: P, mode: libc::mode_t) -> io::Result<()>;

    /// Create a `FileWriter` which provides a `std::io::BufWriter` and then atomically creates
    /// the file at the destination, renaming it over an existing one if necessary.
    fn new_file_writer<'a, P: AsRef<Path>>(
        &'a self,
        destname: P,
        mode: libc::mode_t,
    ) -> io::Result<FileWriter>;

    /// Atomically create or replace the destination file, calling the provided
    /// function to generate the contents.  Note that the contents of the
    /// file will not be explicitly sync'd to disk; if you want to do so you
    /// need to invoke `writer.flush()?; writer.get_ref().sync_all()` for example.
    fn write_file_with<P: AsRef<Path>, F, T, E>(
        &self,
        destname: P,
        mode: libc::mode_t,
        f: F,
    ) -> Result<T, E>
    where
        F: Fn(&mut std::io::BufWriter<std::fs::File>) -> Result<T, E>,
        E: From<io::Error>,
    {
        let mut w = self.new_file_writer(destname, mode)?;
        match f(&mut w.writer) {
            Ok(v) => {
                w.complete()?;
                Ok(v)
            }
            Err(e) => {
                w.abandon();
                Err(e)
            }
        }
    }

    /// Like `write_file_with()` but explicitly synchronizes the target to disk.
    fn write_file_with_sync<P: AsRef<Path>, F, T, E>(
        &self,
        destname: P,
        mode: libc::mode_t,
        f: F,
    ) -> Result<T, E>
    where
        F: Fn(&mut std::io::BufWriter<std::fs::File>) -> Result<T, E>,
        E: From<io::Error>,
    {
        let mut w = self.new_file_writer(destname, mode)?;
        match f(&mut w.writer) {
            Ok(v) => {
                w.complete_with(|f| {
                    f.sync_all()
                })?;
                Ok(v)
            }
            Err(e) => {
                w.abandon();
                Err(e)
            }
        }
    }

    /// Atomically create or replace the destination file with
    /// the provided contents.
    fn write_file_contents<P: AsRef<Path>, C: AsRef<[u8]>>(
        &self,
        destname: P,
        mode: libc::mode_t,
        contents: C,
    ) -> io::Result<()> {
        self.write_file_with(destname, mode, |w| w.write_all(contents.as_ref()))
    }
}

impl OpenatDirExt for openat::Dir {
    fn open_file_optional<P: openat::AsPath>(&self, p: P) -> io::Result<Option<fs::File>> {
        match self.open_file(p) {
            Ok(f) => Ok(Some(f)),
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn remove_file_optional<P: openat::AsPath>(&self, p: P) -> io::Result<()> {
        match self.remove_file(p) {
            Ok(_) => Ok(()),
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

    fn metadata_optional<P: openat::AsPath>(&self, p: P) -> io::Result<Option<openat::Metadata>> {
        match self.metadata(p) {
            Ok(d) => Ok(Some(d)),
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn sub_dir_optional<P: openat::AsPath>(&self, p: P) -> io::Result<Option<openat::Dir>> {
        match self.sub_dir(p) {
            Ok(d) => Ok(Some(d)),
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn get_file_type(&self, e: &openat::Entry) -> io::Result<openat::SimpleType> {
        if let Some(ftype) = e.simple_type() {
            Ok(ftype)
        } else {
            Ok(self.metadata(e.file_name())?.simple_type())
        }
    }

    fn exists<P: openat::AsPath>(&self, p: P) -> io::Result<bool> {
        match self.metadata(p) {
            Ok(_) => Ok(true),
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    Ok(false)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn ensure_dir<P: openat::AsPath>(&self, p: P, mode: libc::mode_t) -> io::Result<()> {
        match self.create_dir(p, mode) {
            Ok(_) => Ok(()),
            Err(e) => {
                if e.kind() == io::ErrorKind::AlreadyExists {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

    fn ensure_dir_all<P: openat::AsPath>(&self, p: P, mode: libc::mode_t) -> io::Result<()> {
        let p = p
            .to_path()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "nul byte in file name"))?;
        let p = p.as_ref();
        // Convert to a `Path` basically just so that we can call `.parent()` on it.
        let p = Path::new(OsStr::from_bytes(p.to_bytes()));
        // Our first system call here is a direct `mkdirat()` - if that succeeds or
        // we get EEXIST then we're done.
        match self.create_dir(p, mode) {
            Ok(_) => {}
            Err(e) => match e.kind() {
                io::ErrorKind::AlreadyExists => {}
                // Otherwise, the expensive path
                io::ErrorKind::NotFound => impl_ensure_dir_all(self, p, mode)?,
                _ => return Err(e),
            },
        }
        Ok(())
    }

    fn new_file_writer<'a, P: AsRef<Path>>(
        &'a self,
        destname: P,
        mode: libc::mode_t,
    ) -> io::Result<FileWriter> {
        let tmpf = self.new_unnamed_file(mode)?;
        let destname = destname.as_ref();
        Ok(FileWriter::new(self, tmpf, destname.to_path_buf()))
    }
}

/// Walk up the path components, creating each directory in turn.  This is
/// pessimistic, assuming no components exist.  But we already handled the
/// optimal case where all components exist above.
pub(crate) fn impl_ensure_dir_all(d: &openat::Dir, p: &Path, mode: libc::mode_t) -> io::Result<()> {
    if let Some(parent) = p.parent() {
        if parent.as_os_str().len() > 0 {
            impl_ensure_dir_all(d, parent, mode)?;
        }
    }
    d.ensure_dir(p, mode)?;
    Ok(())
}

/// A wrapper for atomically replacing a file.  The primary field
/// to access here is the `writer`.  You can also configure the
/// temporary prefix and suffix used for the temporary file before
/// it is moved over the final destination.
///
/// To ensure that errors are handled correctly, you must explicitly
/// invoke either `complete()` or `abandon()`.  Letting this value
/// drop without invoking either of those will panic (unless the
/// thread is already panicing, in which case this requirement is skipped).
pub struct FileWriter<'a> {
    /// Write to the destination file.
    pub writer: std::io::BufWriter<std::fs::File>,
    /// This string will be used as a prefix for the temporary file
    pub tmp_prefix: String,
    /// This string will be used as a suffix for the temporary file
    pub tmp_suffix: String,

    /// The target file name
    destname: PathBuf,
    /// The target directory
    dir: &'a openat::Dir,
    /// Used to ensure that either complete() or abandon() are invoked
    bomb: drop_bomb::DropBomb,
}

impl<'a> FileWriter<'a> {
    fn new(dir: &'a openat::Dir, f: std::fs::File, destname: PathBuf) -> Self {
        Self {
            writer: std::io::BufWriter::new(f),
            destname,
            tmp_prefix: ".tmp.".to_string(),
            tmp_suffix: ".tmp".to_string(),
            dir,
            bomb: drop_bomb::DropBomb::new(
                "FileWriter must be explicitly completed/abandoned to ensure errors are checked",
            ),
        }
    }

    /// Flush any outstanding buffered data and rename the temporary
    /// file into place.  The provided closure will be invoked
    /// on the real underlying file descriptor before it is
    /// renamed into place.  You can use this to change file attributes.
    /// For example, you can change the mode, extended attributes, or invoke
    /// `fchmod()` to change ownership, etc.
    pub fn complete_with<F>(mut self, f: F) -> io::Result<()>
    where
        F: Fn(&fs::File) -> io::Result<()>,
    {
        self.bomb.defuse();
        let fd = self.writer.into_inner()?;
        f(&fd)?;
        let mut rng = rand::thread_rng();
        // Unfortunately there's no linkat(LINKAT_REPLACE) yet, so we need
        // to generate a tempfile as the penultimate step.
        let tmpname = loop {
            let mut tmpname = self.tmp_prefix.clone();
            for _ in 0..8 {
                tmpname.push(rng.sample(rand::distributions::Alphanumeric))
            }
            tmpname.push_str(self.tmp_suffix.as_str());
            match self.dir.link_file_at(&fd, tmpname.as_str()) {
                Ok(()) => {
                    break tmpname;
                }
                Err(e) => {
                    if e.kind() == io::ErrorKind::AlreadyExists {
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            }
        };
        let tmpname = tmpname.as_str();
        // Then rename it into place.
        match self.dir.local_rename(tmpname, &self.destname) {
            Ok(()) => Ok(()),
            Err(e) => {
                // If we failed, clean up
                let _ = self.dir.remove_file(tmpname);
                Err(e)
            }
        }
    }

    /// Flush any outstanding buffered data and rename the temporary
    /// file into place.
    pub fn complete(self) -> io::Result<()> {
        self.complete_with(|_f| Ok(()))
    }

    /// Drop any buffered data and delete the temporary file without
    /// affecting the final destination.
    pub fn abandon(mut self) {
        self.bomb.defuse()
        // Ideally BufWriter would have an `abandon()` too
    }
}

// Our methods take &self, not &mut self matching the other raw
// file methods.  We can't use io::copy because it expects mutable
// references, so just reimplement it here.
pub(crate) fn fallback_file_copy(src: &File, dest: &File) -> io::Result<u64> {
    let mut off: u64 = 0;
    let mut buf = [0u8; 8192];
    loop {
        let n = src.read_at(&mut buf, off)?;
        if n == 0 {
            return Ok(off);
        }
        dest.write_all_at(&buf[0..n], off)?;
        off += n as u64;
    }
}

/// Helper functions for std::fs::File
pub trait FileExt {
    /// Copy the entire contents of `self` to `to`.  This uses operating system
    /// specific fast paths if available.
    fn copy_to(&self, to: &File) -> io::Result<u64>;
}

impl FileExt for File {
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    fn copy_to(&self, to: &File) -> io::Result<u64> {
        fallback_file_copy(self, to)
    }

    // Derived from src/libstd/sys/unix/fs.rs in Rust
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn copy_to(&self, to: &File) -> io::Result<u64> {
        use nix::errno::Errno;
        use nix::fcntl::copy_file_range;
        use std::os::unix::io::AsRawFd;
        use std::sync::atomic::{AtomicBool, Ordering};

        // Kernel prior to 4.5 don't have copy_file_range
        // We store the availability in a global to avoid unnecessary syscalls
        static HAS_COPY_FILE_RANGE: AtomicBool = AtomicBool::new(true);

        let len = self.metadata()?.len();

        let has_copy_file_range = HAS_COPY_FILE_RANGE.load(Ordering::Relaxed);
        let mut written = 0u64;
        while written < len {
            let copy_result = if has_copy_file_range {
                let bytes_to_copy = std::cmp::min(len - written, usize::MAX as u64) as usize;
                // We actually don't have to adjust the offsets,
                // because copy_file_range adjusts the file offset automatically
                let copy_result =
                    copy_file_range(self.as_raw_fd(), None, to.as_raw_fd(), None, bytes_to_copy);
                if let Err(ref copy_err) = copy_result {
                    match copy_err.as_errno() {
                        Some(Errno::ENOSYS) | Some(Errno::EPERM) => {
                            HAS_COPY_FILE_RANGE.store(false, Ordering::Relaxed);
                        }
                        _ => {}
                    }
                }
                copy_result
            } else {
                Err(nix::Error::from_errno(Errno::ENOSYS))
            };
            match copy_result {
                Ok(ret) => written += ret as u64,
                Err(err) => {
                    match err.as_errno() {
                        Some(os_err)
                            if os_err == Errno::ENOSYS
                                || os_err == Errno::EXDEV
                                || os_err == Errno::EINVAL
                                || os_err == Errno::EPERM =>
                        {
                            // Try fallback io::copy if either:
                            // - Kernel version is < 4.5 (ENOSYS)
                            // - Files are mounted on different fs (EXDEV)
                            // - copy_file_range is disallowed, for example by seccomp (EPERM)
                            // - copy_file_range cannot be used with pipes or device nodes (EINVAL)
                            assert_eq!(written, 0);
                            return fallback_file_copy(self, to);
                        }
                        Some(os_err) => return Err(io::Error::from_raw_os_error(os_err as i32)),
                        _ => return Err(io::Error::new(io::ErrorKind::Other, err)),
                    }
                }
            }
        }
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::{error, result};
    use tempfile;

    type Result<T> = result::Result<T, Box<dyn error::Error>>;

    #[test]
    fn open_file_optional() -> Result<()> {
        let td = tempfile::tempdir()?;
        let d = openat::Dir::open(td.path())?;
        assert!(d.open_file_optional("foo")?.is_none());
        d.write_file("foo", 0o644)?.sync_all()?;
        assert!(d.open_file_optional("foo")?.is_some());
        Ok(())
    }

    #[test]
    fn remove_file_optional() -> Result<()> {
        let td = tempfile::tempdir()?;
        let d = openat::Dir::open(td.path())?;
        d.write_file("foo", 0o644)?.sync_all()?;
        assert!(d.open_file_optional("foo")?.is_some());
        d.remove_file_optional("foo")?;
        assert!(d.open_file_optional("foo")?.is_none());
        Ok(())
    }

    #[test]
    fn metadata_optional() -> Result<()> {
        let td = tempfile::tempdir()?;
        let d = openat::Dir::open(td.path())?;
        assert!(d.metadata_optional("foo")?.is_none());
        d.write_file("foo", 0o644)?.sync_all()?;
        assert!(d.metadata_optional("foo")?.is_some());
        Ok(())
    }

    #[test]
    fn get_file_type() -> Result<()> {
        let td = tempfile::tempdir()?;
        let d = openat::Dir::open(td.path())?;
        d.write_file("foo", 0o644)?.sync_all()?;
        for x in d.list_dir(".")? {
            let x = x?;
            assert_eq!("foo", x.file_name());
            let t = d.get_file_type(&x)?;
            assert_eq!(openat::SimpleType::File, t);
        }
        Ok(())
    }

    #[test]
    fn ensure_dir_all() -> Result<()> {
        let td = tempfile::tempdir()?;
        let d = openat::Dir::open(td.path())?;
        let mode = 0o755;
        let p = Path::new("foo/bar/baz");
        d.ensure_dir_all(p, mode)?;
        assert_eq!(d.metadata(p)?.stat().st_mode & !libc::S_IFMT, mode);
        d.ensure_dir_all(p, mode)?;
        d.ensure_dir_all("foo/bar", mode)?;
        d.ensure_dir_all("foo", mode)?;
        d.ensure_dir_all("bar", 0o700)?;
        assert_eq!(d.metadata("bar")?.stat().st_mode & !libc::S_IFMT, 0o700);
        Ok(())
    }

    fn find_test_file(tempdir: &Path) -> Result<PathBuf> {
        for p in ["/proc/self/exe", "/usr/bin/bash"].iter() {
            let p = Path::new(p);
            if p.exists() {
                return Ok(p.into());
            }
        }
        let fallback = tempdir.join("testfile-fallback");
        std::fs::write(&fallback, "some test data")?;
        Ok(fallback)
    }

    #[test]
    fn copy_fallback() -> Result<()> {
        use std::io::Read;
        let td = tempfile::tempdir()?;
        let src_p = find_test_file(td.path())?;
        let dest_p = td.path().join("bash");
        {
            let src = File::open(&src_p)?;
            let dest = File::create(&dest_p)?;
            let _ = fallback_file_copy(&src, &dest)?;
        }
        let mut src = File::open(&src_p)?;
        let mut srcbuf = Vec::new();
        let _ = src.read_to_end(&mut srcbuf)?;
        let mut destbuf = Vec::new();
        let mut dest = File::open(&dest_p)?;
        let _ = dest.read_to_end(&mut destbuf)?;
        assert_eq!(srcbuf.len(), destbuf.len());
        assert_eq!(&srcbuf, &destbuf);
        Ok(())
    }
}
