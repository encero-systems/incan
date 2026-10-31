//! Bounded, read-only views that retain their mapping without copying mapped bytes.
//!
//! The owner anchor is explicit: dropping a mapping handle cannot unmap bytes still held by a view. This is not a
//! lexical borrow, and a caller retaining a view also retains the whole virtual mapping. The backing file must remain
//! immutable until the last handle or view is dropped; neither a read-only descriptor nor this anchor prevents an
//! unrelated process from truncating or modifying the inode. A view lends its bytes to native readers through
//! `AsRef<[u8]>`, so hashing or comparing mapped bytes needs no copy.

use std::fs::File;
use std::ops::Range;
use std::sync::Arc;

use memmap2::Mmap;

/// A read-only mapping shared with its bounded views.
#[derive(Clone, Debug)]
pub struct MappedFile {
    mapping: Arc<Mmap>,
}

impl MappedFile {
    /// Map an immutable file without reading its contents into an owned byte buffer.
    ///
    /// The backing inode must not be modified or truncated until this mapping and every view derived from it have been
    /// dropped; `std.fs` states this as the contract of `map_read`. Replacing the path with another inode does not
    /// change this mapping.
    pub fn open(path: &str) -> Result<Self, String> {
        let file = File::open(path).map_err(|error| error.to_string())?;
        // SAFETY: the inode is not modified or truncated while any handle or view of this mapping lives. That is the
        // documented contract of every caller; the bakery store meets it because its objects are content-named and
        // never rewritten in place.
        let mapping = unsafe { Mmap::map(&file) }.map_err(|error| error.to_string())?;
        if mapping.len() > i64::MAX as usize {
            return Err("mapping length exceeds the supported signed offset range".into());
        }
        Ok(Self {
            mapping: Arc::new(mapping),
        })
    }

    /// The mapped byte length, representable by Incan's signed `int`.
    pub fn len(&self) -> i64 {
        self.mapping.len() as i64
    }

    /// Whether the mapping has no bytes.
    pub fn is_empty(&self) -> bool {
        self.mapping.is_empty()
    }

    /// Retain a bounded range without copying its contents or exposing an unanchored pointer.
    pub fn view(&self, offset: i64, length: i64) -> Option<MappedView> {
        let range = checked_range(self.mapping.len(), offset, length)?;
        Some(MappedView {
            mapping: Arc::clone(&self.mapping),
            range,
        })
    }
}

/// A zero-copy range with its own shared lifetime anchor, not an independently allocated byte buffer.
#[derive(Clone, Debug)]
pub struct MappedView {
    mapping: Arc<Mmap>,
    range: Range<usize>,
}

impl MappedView {
    /// The view's length, not the size of its backing mapping.
    pub fn len(&self) -> i64 {
        self.range.len() as i64
    }

    /// Whether this bounded view contains no bytes.
    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }

    /// Borrow the view's bytes for no longer than the owner anchor is borrowed.
    pub fn as_bytes(&self) -> &[u8] {
        &self.mapping[self.range.clone()]
    }

    /// Slice relative to this view, retaining the same backing mapping and enforcing this view's bounds.
    pub fn slice(&self, offset: i64, length: i64) -> Option<Self> {
        let relative = checked_range(self.range.len(), offset, length)?;
        let start = self.range.start.checked_add(relative.start)?;
        let end = self.range.start.checked_add(relative.end)?;
        Some(Self {
            mapping: Arc::clone(&self.mapping),
            range: start..end,
        })
    }

    /// Explicitly copy the view into an independently owned byte buffer.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }

    /// Read a byte at a view-relative offset without allocating or copying a byte buffer.
    pub fn read_u8(&self, offset: i64) -> Option<u8> {
        let range = checked_range(self.range.len(), offset, 1)?;
        self.as_bytes().get(range.start).copied()
    }

    /// Read a signed byte at a view-relative offset.
    pub fn read_i8(&self, offset: i64) -> Option<i8> {
        Some(self.read_u8(offset)? as i8)
    }

    /// Obtain a scalar's fixed-width bytes on the stack, without alignment assumptions or heap allocation.
    fn scalar<const N: usize>(&self, offset: i64) -> Option<[u8; N]> {
        let range = checked_range(self.range.len(), offset, N as i64)?;
        self.as_bytes().get(range)?.try_into().ok()
    }
}

/// A view lends its bytes, in place, to native APIs that read `AsRef<[u8]>`.
impl AsRef<[u8]> for MappedView {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Generate endian-specific scalar readers without unsafe native-structure casts.
macro_rules! little_endian {
    ($($method:ident: $scalar:ty),+ $(,)?) => {
        impl MappedView {
            $(
                #[doc = concat!("Read a little-endian `", stringify!($scalar), "` at a view-relative offset; return `None` outside the view.")]
                pub fn $method(&self, offset: i64) -> Option<$scalar> {
                    Some(<$scalar>::from_le_bytes(self.scalar(offset)?))
                }
            )+
        }
    };
}

little_endian! {
    read_u16_le: u16,
    read_u32_le: u32,
    read_u64_le: u64,
    read_i16_le: i16,
    read_i32_le: i32,
    read_i64_le: i64,
    read_f32_le: f32,
    read_f64_le: f64,
}

/// Reject negative, overflowing, or out-of-bounds ranges, permitting an empty range at the end.
fn checked_range(total: usize, offset: i64, length: i64) -> Option<Range<usize>> {
    let start = usize::try_from(offset).ok()?;
    let length = usize::try_from(length).ok()?;
    let end = start.checked_add(length)?;
    (end <= total).then_some(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build one mapping over an immutable temporary file retained by the test.
    fn mapped(bytes: &[u8]) -> Result<(tempfile::NamedTempFile, MappedFile), Box<dyn std::error::Error>> {
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(bytes)?;
        file.flush()?;
        let path = file.path().to_str().ok_or("test path is not UTF-8")?;
        // The test retains the file and never writes or truncates its inode after this call.
        let mapping = MappedFile::open(path)?;
        Ok((file, mapping))
    }

    /// Views share addresses and keep the mapping alive after the original handle and parent view are dropped.
    #[test]
    fn shared_backing_survives_handle_and_parent() -> Result<(), Box<dyn std::error::Error>> {
        let (_file, mapping) = mapped(b"0123456789")?;
        let weak = Arc::downgrade(&mapping.mapping);
        let parent = mapping.view(2, 6).ok_or("missing parent")?;
        let child = parent.slice(1, 3).ok_or("missing child")?;
        assert!(Arc::ptr_eq(&mapping.mapping, &child.mapping));
        assert_eq!(child.as_bytes().as_ptr(), mapping.mapping[3..].as_ptr());
        let lent: &[u8] = child.as_ref();
        assert_eq!(lent.as_ptr(), mapping.mapping[3..].as_ptr());
        drop(mapping);
        drop(parent);
        assert_eq!(child.as_bytes(), b"345");
        assert!(weak.upgrade().is_some());
        drop(child);
        assert!(weak.upgrade().is_none());
        Ok(())
    }

    /// Slicing checks view-relative bounds, negative values and extreme signed offsets without overflow.
    #[test]
    fn bounded_ranges() -> Result<(), Box<dyn std::error::Error>> {
        let (_file, mapping) = mapped(b"0123456789")?;
        let view = mapping.view(2, 4).ok_or("missing view")?;
        assert_eq!(view.slice(4, 0).ok_or("missing empty view")?.len(), 0);
        for (offset, length) in [(-1, 0), (0, -1), (5, 0), (3, 2), (i64::MAX, 1), (1, i64::MAX)] {
            assert!(view.slice(offset, length).is_none());
        }
        assert!(mapping.view(i64::MAX, i64::MAX).is_none());
        assert_eq!(
            mapping.view(2, 4).map(|copied| copied.to_bytes()),
            Some(b"2345".to_vec())
        );
        Ok(())
    }

    /// Unaligned endian reads preserve unsigned high bits and signed/float values without widening the view.
    #[test]
    fn scalar_reads() -> Result<(), Box<dyn std::error::Error>> {
        let (_file, mapping) = mapped(&[0, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 128, 63])?;
        let view = mapping.view(1, 12).ok_or("missing view")?;
        assert_eq!(view.read_u8(0), Some(u8::MAX));
        assert_eq!(view.read_i8(0), Some(-1));
        assert_eq!(view.read_u16_le(0), Some(u16::MAX));
        assert_eq!(view.read_u32_le(0), Some(u32::MAX));
        assert_eq!(view.read_u64_le(0), Some(u64::MAX));
        assert_eq!(view.read_i16_le(0), Some(-1));
        assert_eq!(view.read_i32_le(0), Some(-1));
        assert_eq!(view.read_i64_le(0), Some(-1));
        assert_eq!(view.read_f32_le(8), Some(1.0));
        assert!(view.read_u64_le(5).is_none());
        assert!(view.read_u8(-1).is_none());
        let (_double_file, double_mapping) = mapped(&1.5_f64.to_le_bytes())?;
        assert_eq!(
            double_mapping.view(0, 8).ok_or("missing double")?.read_f64_le(0),
            Some(1.5)
        );
        Ok(())
    }
}
