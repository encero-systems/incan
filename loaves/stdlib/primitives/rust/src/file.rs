//! Concrete host-file operations for checked Incan ownership boundaries.
//!
//! This adapter is removable #2500 debt. The Incan compiler does not yet admit native results that pass through the
//! borrowed adapters returned by `Read::by_ref` and `Read::take` inside an explicit-input body. Keeping the concrete
//! `Read` and `Write` calls here leaves the public resource surface in Incan while #2500 learns to prove that body.

use std::fs::{File as RustFile, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};

/// An owned host file released by Rust drop when its Incan owner leaves scope.
#[derive(Debug)]
pub struct File {
    inner: RustFile,
}

impl File {
    /// Open an owned read-only handle.
    pub fn open_read(path: &str) -> Result<Self, String> {
        RustFile::open(path)
            .map(|inner| Self { inner })
            .map_err(|error| error.to_string())
    }

    /// Open an owned handle from explicit host flags without applying higher-level mode policy.
    pub fn open_options(
        path: &str,
        readable: bool,
        writable: bool,
        appendable: bool,
        truncating: bool,
        creating: bool,
        creating_new: bool,
    ) -> Result<Self, String> {
        let mut options = OpenOptions::new();
        options
            .read(readable)
            .write(writable)
            .append(appendable)
            .truncate(truncating)
            .create(creating)
            .create_new(creating_new);
        options
            .open(path)
            .map(|inner| Self { inner })
            .map_err(|error| error.to_string())
    }

    /// Read at most `max_bytes`, allowing a short read and returning an empty vector at EOF.
    pub fn read_chunk(&mut self, max_bytes: i64) -> Result<Vec<u8>, String> {
        let limit = u64::try_from(max_bytes)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| "max_bytes must be greater than zero".to_owned())?;
        let mut chunk = Vec::new();
        Read::by_ref(&mut self.inner)
            .take(limit)
            .read_to_end(&mut chunk)
            .map_err(|error| error.to_string())?;
        Ok(chunk)
    }

    /// Read every remaining byte from the current cursor.
    pub fn read_remaining(&mut self) -> Result<Vec<u8>, String> {
        let mut data = Vec::new();
        self.inner.read_to_end(&mut data).map_err(|error| error.to_string())?;
        Ok(data)
    }

    /// Write every byte and return the accepted byte count.
    pub fn write_all(&mut self, data: &[u8]) -> Result<i64, String> {
        self.inner.write_all(data).map_err(|error| error.to_string())?;
        i64::try_from(data.len()).map_err(|_| "written byte count exceeds the supported range".to_owned())
    }

    /// Return the current byte cursor position.
    pub fn tell(&mut self) -> Result<i64, String> {
        let position = self.inner.stream_position().map_err(|error| error.to_string())?;
        i64::try_from(position).map_err(|_| "file position exceeds the supported range".to_owned())
    }

    /// Move the cursor relative to the start, current position or end and return its new position.
    pub fn seek(&mut self, offset: i64, whence: i64) -> Result<i64, String> {
        let position = if whence == 0 {
            let absolute = u64::try_from(offset).map_err(|_| "absolute seek offset must not be negative".to_owned())?;
            SeekFrom::Start(absolute)
        } else if whence == 1 {
            SeekFrom::Current(offset)
        } else {
            SeekFrom::End(offset)
        };
        let next = self.inner.seek(position).map_err(|error| error.to_string())?;
        i64::try_from(next).map_err(|_| "file position exceeds the supported range".to_owned())
    }

    /// Flush buffered writes to the host handle.
    pub fn flush(&mut self) -> Result<(), String> {
        self.inner.flush().map_err(|error| error.to_string())
    }

    /// Ask the host to synchronize file data and metadata to durable storage.
    pub fn sync(&mut self) -> Result<(), String> {
        self.inner.sync_all().map_err(|error| error.to_string())
    }

    /// Ask the host to synchronize file data without requiring all metadata.
    pub fn sync_data(&mut self) -> Result<(), String> {
        self.inner.sync_data().map_err(|error| error.to_string())
    }
}

impl Read for File {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buffer)
    }
}

impl Write for File {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.inner.write(data)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::File;
    use std::fs;

    /// Bounded reads permit short final chunks, report EOF and keep independent cursors.
    #[test]
    fn bounded_reads_and_independent_cursors() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("source.incn");
        fs::write(&path, b"abcdefg")?;
        let text = path.to_str().ok_or("test path is not UTF-8")?;
        let mut first = File::open_read(text)?;
        let mut second = File::open_read(text)?;
        assert_eq!(first.read_chunk(3)?, b"abc");
        assert_eq!(first.read_chunk(8)?, b"defg");
        assert_eq!(first.read_chunk(1)?, b"");
        assert_eq!(second.read_chunk(1)?, b"a");
        Ok(())
    }

    /// Write, seek and remaining-read operations share one owned host cursor.
    #[test]
    fn writes_seek_and_read_remaining() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("resource.bin");
        let text = path.to_str().ok_or("test path is not UTF-8")?;
        let mut file = File::open_options(text, true, true, false, true, true, false)?;
        assert_eq!(file.write_all(b"payload")?, 7);
        assert_eq!(file.seek(0, 0)?, 0);
        assert_eq!(file.read_remaining()?, b"payload");
        file.flush()?;
        Ok(())
    }
}
