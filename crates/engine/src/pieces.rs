//! A song whole on the disk, kept in one file or several one after the other (media3's cache keeps a
//! song in pieces of a few megabytes), read as one stream in large sequential reads: for measuring a
//! song ahead, which reads it once from its first byte to its last.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;

use symphonia::core::io::MediaSource;

/// How much is read from the disk at a time. The demuxer asks for a few kilobytes per call; each of
/// those is a copy out of this, and the disk is read about twice a second at most.
pub const READ: usize = 512 * 1024;

/// The pieces of one song in order, each with its length, read through one buffer made once.
pub struct Pieces<F = File> {
    parts: Vec<(F, u64)>,
    len: u64,
    /// Where the reader is in the song.
    pos: u64,
    /// The bytes read ahead (the first `filled` of `buf`), and where in the song they start.
    buf: Box<[u8]>,
    filled: usize,
    buf_at: u64,
    /// Which part is where, and where that part's own cursor stands: a read that follows the one
    /// before is not preceded by a seek.
    part_at: Option<(usize, u64)>,
}

impl Pieces<File> {
    /// Opens every file at once: a piece the cache lets go while the song is read stays readable.
    pub fn open(files: &[PathBuf]) -> io::Result<Pieces<File>> {
        let mut parts = Vec::with_capacity(files.len());
        for f in files {
            let file = File::open(f)?;
            let len = file.metadata()?.len();
            parts.push((file, len));
        }
        Ok(Pieces::new(parts))
    }
}

impl<F: Read + Seek> Pieces<F> {
    pub fn new(parts: Vec<(F, u64)>) -> Pieces<F> {
        let len = parts.iter().map(|(_, l)| l).sum();
        Pieces { parts, len, pos: 0, buf: vec![0; READ].into_boxed_slice(), filled: 0, buf_at: 0, part_at: None }
    }

    /// All of the song's bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Fills the buffer from `self.pos` on, across as many pieces as it takes.
    fn fill(&mut self) -> io::Result<()> {
        self.filled = 0;
        self.buf_at = self.pos;
        let want = READ.min((self.len - self.pos) as usize);
        let mut got = 0;
        let (mut part, mut start) = self.part_of(self.pos);
        while got < want && part < self.parts.len() {
            let offset = self.pos + got as u64 - start;
            let (file, len) = &mut self.parts[part];
            if self.part_at != Some((part, offset)) {
                file.seek(SeekFrom::Start(offset))?;
            }
            let take = (want - got).min((*len - offset) as usize);
            let n = file.read(&mut self.buf[got..got + take])?;
            if n == 0 {
                // The file is shorter than it said: the song ends here.
                break;
            }
            got += n;
            self.part_at = Some((part, offset + n as u64));
            if offset + n as u64 == *len {
                start += *len;
                part += 1;
            }
        }
        self.filled = got;
        Ok(())
    }

    /// The piece that holds byte `at`, and where that piece starts in the song.
    fn part_of(&self, at: u64) -> (usize, u64) {
        let mut start = 0;
        for (i, (_, len)) in self.parts.iter().enumerate() {
            if at < start + len {
                return (i, start);
            }
            start += len;
        }
        (self.parts.len(), start)
    }
}

impl<F: Read + Seek> Read for Pieces<F> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.pos >= self.len {
            return Ok(0);
        }
        let end = self.buf_at + self.filled as u64;
        if self.pos < self.buf_at || self.pos >= end {
            self.fill()?;
            if self.filled == 0 {
                return Ok(0);
            }
        }
        let from = (self.pos - self.buf_at) as usize;
        let n = out.len().min(self.filled - from);
        out[..n].copy_from_slice(&self.buf[from..from + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl<F: Read + Seek> Seek for Pieces<F> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let at = match to {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if at < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "a seek before the start"));
        }
        self.pos = at as u64;
        Ok(self.pos)
    }
}

impl<F: Read + Seek + Send + Sync> MediaSource for Pieces<F> {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    /// A piece that notes the size of every read made of it.
    struct Counted {
        inner: Cursor<Vec<u8>>,
        reads: Arc<Mutex<Vec<usize>>>,
    }

    impl Read for Counted {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.reads.lock().unwrap().push(n);
            Ok(n)
        }
    }

    impl Seek for Counted {
        fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
            self.inner.seek(to)
        }
    }

    fn song(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn cut(bytes: &[u8], at: &[usize], reads: &Arc<Mutex<Vec<usize>>>) -> Pieces<Counted> {
        let mut parts = Vec::new();
        let mut from = 0;
        for &to in at.iter().chain(std::iter::once(&bytes.len())) {
            parts.push((Counted { inner: Cursor::new(bytes[from..to].to_vec()), reads: reads.clone() }, (to - from) as u64));
            from = to;
        }
        Pieces::new(parts)
    }

    #[test]
    fn pieces_read_as_one_song_in_large_reads_however_small_the_reader_s_steps() {
        let bytes = song(3 * READ + 12_345);
        let reads = Arc::new(Mutex::new(Vec::new()));
        // Cut where media3 would, and once more at an odd place.
        let mut p = cut(&bytes, &[1_000_000, 1_048_576 + 7], &reads);
        let mut out = Vec::new();
        let mut step = [0u8; 4096];
        loop {
            let n = p.read(&mut step).unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&step[..n]);
        }
        assert!(out == bytes, "every byte, in order");
        let reads = reads.lock().unwrap();
        // One read per buffer's worth, plus one more where a buffer runs over a piece's end.
        assert!(reads.len() <= bytes.len().div_ceil(READ) + 2, "{} reads for {} bytes: {:?}", reads.len(), bytes.len(), *reads);
        let small = reads.iter().filter(|&&n| n < 64 * 1024).count();
        assert!(small <= 3, "only a piece's end or the song's is read short: {:?}", *reads);
    }

    #[test]
    fn a_seek_reads_from_there_and_a_seek_inside_what_is_read_costs_nothing() {
        let bytes = song(2 * READ);
        let reads = Arc::new(Mutex::new(Vec::new()));
        let mut p = cut(&bytes, &[READ / 2], &reads);
        let mut b = [0u8; 100];
        p.seek(SeekFrom::End(-100)).unwrap();
        p.read_exact(&mut b).unwrap();
        assert_eq!(&b[..], &bytes[bytes.len() - 100..], "the end, as an MP4's boxes are looked for");
        p.seek(SeekFrom::Start(10)).unwrap();
        p.read_exact(&mut b).unwrap();
        assert_eq!(&b[..], &bytes[10..110]);
        let before = reads.lock().unwrap().len();
        p.seek(SeekFrom::Start(1000)).unwrap();
        p.read_exact(&mut b).unwrap();
        assert_eq!(&b[..], &bytes[1000..1100]);
        assert_eq!(reads.lock().unwrap().len(), before, "already read");
        assert_eq!(p.byte_len(), Some(bytes.len() as u64));
    }
}
