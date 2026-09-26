//! Block-level I/O helpers.
//!
//! Raw disk handles on macOS (`/dev/rdiskN`) and Windows (`\\.\PhysicalDriveN`)
//! reject reads and writes that are not aligned to the sector size. Everything
//! that touches a raw device therefore goes through these wrappers, which only
//! ever issue aligned I/O to the underlying handle.

use std::io::{self, Read, Seek, SeekFrom, Write};

/// Largest sector size we expect on removable media. Aligning every raw read
/// to this satisfies both 512-byte and 4Kn devices.
pub const MAX_SECTOR: u64 = 4096;

/// Random-access reader over a disk or disk image with a known size.
pub struct Disk<R> {
    inner: R,
    size: u64,
    align: u64,
}

impl<R: Read + Seek> Disk<R> {
    pub fn new(inner: R, size: u64) -> Self {
        Disk { inner, size, align: MAX_SECTOR }
    }

    /// Open a reader whose size is discovered by seeking to the end. Works for
    /// image files and for block devices on Linux and macOS.
    pub fn from_seekable(mut inner: R) -> io::Result<Self> {
        let size = inner.seek(SeekFrom::End(0))?;
        Ok(Disk::new(inner, size))
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// Read `len` bytes at `offset`, issuing only aligned reads underneath.
    pub fn read_at(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let end = offset
            .checked_add(len as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "offset overflow"))?;
        if end > self.size {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("read of {len} bytes at {offset} past end of {}-byte disk", self.size),
            ));
        }
        let start_aligned = offset - offset % self.align;
        let end_aligned = end.div_ceil(self.align) * self.align;
        let end_aligned = end_aligned.min(self.size.div_ceil(self.align) * self.align);
        let mut buf = vec![0u8; (end_aligned - start_aligned) as usize];
        self.inner.seek(SeekFrom::Start(start_aligned))?;
        read_full(&mut self.inner, &mut buf)?;
        let from = (offset - start_aligned) as usize;
        Ok(buf[from..from + len].to_vec())
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

/// Like `read_exact`, but a short read at end of file zero-fills instead of
/// failing. Image files are not always a whole number of 4 KiB blocks.
fn read_full<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    buf[filled..].fill(0);
    Ok(())
}

/// A window onto part of a larger stream, e.g. one partition of a disk.
pub struct Slice<T> {
    inner: T,
    start: u64,
    len: u64,
    pos: u64,
}

impl<T: Seek> Slice<T> {
    pub fn new(inner: T, start: u64, len: u64) -> Self {
        Slice { inner, start, len, pos: 0 }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: Read + Seek> Read for Slice<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.len.saturating_sub(self.pos);
        let n = (buf.len() as u64).min(remaining) as usize;
        if n == 0 {
            return Ok(0);
        }
        self.inner.seek(SeekFrom::Start(self.start + self.pos))?;
        let got = self.inner.read(&mut buf[..n])?;
        self.pos += got as u64;
        Ok(got)
    }
}

impl<T: Write + Seek> Write for Slice<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let remaining = self.len.saturating_sub(self.pos);
        if remaining == 0 && !buf.is_empty() {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "write past end of slice"));
        }
        let n = (buf.len() as u64).min(remaining) as usize;
        self.inner.seek(SeekFrom::Start(self.start + self.pos))?;
        let wrote = self.inner.write(&buf[..n])?;
        self.pos += wrote as u64;
        Ok(wrote)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<T> Seek for Slice<T> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new = match pos {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if new < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = new as u64;
        Ok(self.pos)
    }
}

/// Makes an unaligned `Read + Write + Seek` consumer (such as the `fatfs`
/// formatter) safe to run against a raw device that only accepts whole
/// sectors. Unaligned writes become read-modify-write of the covering sectors.
pub struct AlignedIo<T> {
    inner: T,
    sector: u64,
    pos: u64,
    len: u64,
    /// One-sector write-back cache so sequential small writes don't each cost
    /// a read-modify-write round trip.
    cache: Option<(u64, Vec<u8>, bool)>,
}

impl<T: Read + Write + Seek> AlignedIo<T> {
    pub fn new(inner: T, sector: u64, len: u64) -> Self {
        assert!(sector.is_power_of_two() && sector >= 512);
        AlignedIo { inner, sector, pos: 0, len, cache: None }
    }

    fn load(&mut self, sector_index: u64) -> io::Result<()> {
        if let Some((idx, _, _)) = &self.cache {
            if *idx == sector_index {
                return Ok(());
            }
        }
        self.flush_cache()?;
        let mut buf = vec![0u8; self.sector as usize];
        self.inner.seek(SeekFrom::Start(sector_index * self.sector))?;
        read_full(&mut self.inner, &mut buf)?;
        self.cache = Some((sector_index, buf, false));
        Ok(())
    }

    fn flush_cache(&mut self) -> io::Result<()> {
        if let Some((idx, buf, dirty)) = &mut self.cache {
            if *dirty {
                self.inner.seek(SeekFrom::Start(*idx * self.sector))?;
                self.inner.write_all(buf)?;
                *dirty = false;
            }
        }
        Ok(())
    }

    pub fn into_inner(mut self) -> io::Result<T> {
        self.flush_cache()?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<T: Read + Write + Seek> Read for AlignedIo<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(self.len - self.pos) as usize;
        let sector_index = self.pos / self.sector;
        let within = (self.pos % self.sector) as usize;
        // Whole aligned sectors with no pending cached write can go straight through.
        if within == 0 && want as u64 >= self.sector {
            self.flush_cache()?;
            let n = (want as u64 / self.sector * self.sector) as usize;
            self.inner.seek(SeekFrom::Start(self.pos))?;
            read_full(&mut self.inner, &mut buf[..n])?;
            self.pos += n as u64;
            return Ok(n);
        }
        self.load(sector_index)?;
        let (_, cached, _) = self.cache.as_ref().expect("loaded");
        let n = want.min(self.sector as usize - within);
        buf[..n].copy_from_slice(&cached[within..within + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl<T: Read + Write + Seek> Write for AlignedIo<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.pos >= self.len {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "write past end of device"));
        }
        let want = (buf.len() as u64).min(self.len - self.pos) as usize;
        let sector_index = self.pos / self.sector;
        let within = (self.pos % self.sector) as usize;
        if within == 0 && want as u64 >= self.sector {
            // Drop a cached copy of any sector we are about to overwrite.
            let n = (want as u64 / self.sector * self.sector) as usize;
            if let Some((idx, _, _)) = &self.cache {
                let first = sector_index;
                let last = sector_index + n as u64 / self.sector;
                if *idx >= first && *idx < last {
                    self.cache = None;
                }
            }
            self.inner.seek(SeekFrom::Start(self.pos))?;
            self.inner.write_all(&buf[..n])?;
            self.pos += n as u64;
            return Ok(n);
        }
        self.load(sector_index)?;
        let n = want.min(self.sector as usize - within);
        let (_, cached, dirty) = self.cache.as_mut().expect("loaded");
        cached[within..within + n].copy_from_slice(&buf[..n]);
        *dirty = true;
        self.pos += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_cache()?;
        self.inner.flush()
    }
}

impl<T: Read + Write + Seek> Seek for AlignedIo<T> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new = match pos {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if new < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = new as u64;
        Ok(self.pos)
    }
}

pub(crate) fn le_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

pub(crate) fn le_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

pub(crate) fn le_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

pub(crate) fn be_u16(b: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([b[off], b[off + 1]])
}

pub(crate) fn be_u32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes(b[off..off + 4].try_into().unwrap())
}

#[allow(dead_code)]
pub(crate) fn be_u64(b: &[u8], off: usize) -> u64 {
    u64::from_be_bytes(b[off..off + 8].try_into().unwrap())
}

/// CRC-32 (IEEE 802.3), as used by GPT headers.
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        t
    });
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Backing store that fails the test on any unaligned access.
    struct StrictDevice {
        data: Vec<u8>,
        pos: u64,
        sector: u64,
    }

    impl Read for StrictDevice {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            assert_eq!(self.pos % self.sector, 0, "unaligned read offset {}", self.pos);
            assert_eq!(buf.len() as u64 % self.sector, 0, "unaligned read len {}", buf.len());
            let start = self.pos as usize;
            let n = buf.len().min(self.data.len() - start);
            buf[..n].copy_from_slice(&self.data[start..start + n]);
            self.pos += n as u64;
            Ok(n)
        }
    }

    impl Write for StrictDevice {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            assert_eq!(self.pos % self.sector, 0, "unaligned write offset {}", self.pos);
            assert_eq!(buf.len() as u64 % self.sector, 0, "unaligned write len {}", buf.len());
            let start = self.pos as usize;
            self.data[start..start + buf.len()].copy_from_slice(buf);
            self.pos += buf.len() as u64;
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Seek for StrictDevice {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            if let SeekFrom::Start(p) = pos {
                self.pos = p;
            } else {
                panic!("only absolute seeks expected");
            }
            Ok(self.pos)
        }
    }

    #[test]
    fn aligned_io_turns_small_writes_into_sector_writes() {
        let dev = StrictDevice { data: vec![0; 8192], pos: 0, sector: 512 };
        let mut io = AlignedIo::new(dev, 512, 8192);
        io.seek(SeekFrom::Start(510)).unwrap();
        io.write_all(&[0x55, 0xAA, 1, 2, 3]).unwrap();
        io.seek(SeekFrom::Start(1024)).unwrap();
        io.write_all(&[9u8; 1536]).unwrap();
        io.seek(SeekFrom::Start(509)).unwrap();
        let mut back = [0u8; 5];
        io.read_exact(&mut back).unwrap();
        assert_eq!(back, [0, 0x55, 0xAA, 1, 2]);
        let dev = io.into_inner().unwrap();
        assert_eq!(&dev.data[510..515], &[0x55, 0xAA, 1, 2, 3]);
        assert!(dev.data[1024..2560].iter().all(|&b| b == 9));
        assert_eq!(dev.data[2560], 0);
    }

    #[test]
    fn disk_read_at_handles_unaligned_ranges() {
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let mut disk = Disk::new(Cursor::new(data.clone()), data.len() as u64);
        assert_eq!(disk.read_at(4095, 3).unwrap(), data[4095..4098].to_vec());
        assert_eq!(disk.read_at(9990, 10).unwrap(), data[9990..10000].to_vec());
        assert!(disk.read_at(9995, 10).is_err());
    }

    #[test]
    fn slice_confines_reads_and_writes() {
        let mut backing = Cursor::new(vec![0u8; 100]);
        {
            let mut s = Slice::new(&mut backing, 10, 20);
            s.write_all(&[1u8; 20]).unwrap();
            assert!(s.write(&[1]).is_err());
        }
        let v = backing.into_inner();
        assert_eq!(v[9], 0);
        assert_eq!(v[10], 1);
        assert_eq!(v[29], 1);
        assert_eq!(v[30], 0);
    }

    #[test]
    fn crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
