//! Reading log files by content.
//!
//! A line is identified by the content it belongs to and its byte offset in
//! that content: the fingerprint hashes the first line of the file, the offset
//! is the start of the line in the decompressed stream. The path is not part of
//! it. A file renamed by a rotation, copied by copytruncate or compressed to
//! `.gz` is the same content, continues from the position of the file it came
//! from, and a read that overlaps an earlier one replaces those documents.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

use flate2::read::MultiGzDecoder;
use sha2::{Digest, Sha256};

use crate::state::{FileRow, Snapshot};

/// How much of the start of a file is read to find its first line.
const FINGERPRINT_PROBE: usize = 4096;

/// Hex characters of the fingerprint that every document carries.
pub const DOC_FINGERPRINT_LEN: usize = 16;

/// Longest line that is read, a longer one cannot be a log line and is skipped.
const MAX_READ_LINE: usize = 1 << 20;

/// Whether a file has to be read again, given its stored row. A file with the
/// same size and modification time was read up to its end already.
pub fn needs_sync(size: u64, mtime_ns: i64, row: Option<&FileRow>) -> bool {
    match row {
        None => true,
        Some(r) => !(r.size == size && r.mtime_ns == mtime_ns),
    }
}

/// Modification time in nanoseconds since the epoch.
pub fn mtime_ns(meta: &std::fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as i64)
}

/// Whether the file has to be decompressed: it is named `.gz` and starts with
/// the gzip magic number. Any other file is read as text.
pub fn is_gzip(file: &mut File, path: &Path) -> bool {
    let named = path.to_string_lossy().to_lowercase().ends_with(".gz");
    if !named {
        return false;
    }
    let mut magic = [0u8; 2];
    let ok = file.seek(SeekFrom::Start(0)).is_ok() && file.read_exact(&mut magic).is_ok();
    ok && magic == [0x1f, 0x8b]
}

/// Size of the uncompressed content from the gzip trailer, modulo 2^32.
fn gzip_content_size(file: &mut File, size: u64) -> u32 {
    if size < 4 {
        return 0;
    }
    let mut trailer = [0u8; 4];
    if file.seek(SeekFrom::Start(size - 4)).is_err() || file.read_exact(&mut trailer).is_err() {
        return 0;
    }
    u32::from_le_bytes(trailer)
}

/// Estimate of the decompressed size of a file, for progress.
pub fn content_size_estimate(path: &Path, size: u64) -> u64 {
    let Ok(mut file) = File::open(path) else { return size };
    if is_gzip(&mut file, path) {
        let isize = u64::from(gzip_content_size(&mut file, size));
        return isize.max(size);
    }
    size
}

/// A reader of the content of a file from an offset. A plain file is read up to
/// the size it had when it was opened, so the stored position never passes
/// what was indexed.
pub fn open_content(path: &Path, size: u64, compressed: bool, offset: u64) -> io::Result<Box<dyn Read + Send>> {
    let mut file = File::open(path)?;
    if !compressed {
        let offset = offset.min(size);
        file.seek(SeekFrom::Start(offset))?;
        return Ok(Box::new(file.take(size - offset)));
    }
    let mut decoder = MultiGzDecoder::new(file.take(size));
    if offset > 0 {
        let mut limited = (&mut decoder).take(offset);
        io::copy(&mut limited, &mut io::sink())?;
    }
    Ok(Box::new(decoder))
}

/// Hex fingerprint of the first line of a content. `None` while the first line
/// is not complete yet or the content has no text.
pub fn fingerprint_of(mut stream: impl Read, compressed: bool) -> io::Result<Option<String>> {
    let mut buf = vec![0u8; FINGERPRINT_PROBE];
    let mut n = 0;
    let mut ended = false;
    while n < buf.len() {
        match stream.read(&mut buf[n..]) {
            Ok(0) => {
                ended = true;
                break;
            }
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                ended = true;
                break;
            }
            Err(e) => return Err(e),
        }
    }
    let mut data = &buf[..n];
    while let Some((first, rest)) = data.split_first() {
        if *first == b'\r' || *first == b'\n' {
            data = rest;
        } else {
            break;
        }
    }
    if data.is_empty() {
        return Ok(None);
    }
    let line = match data.iter().position(|b| *b == b'\n') {
        Some(i) => &data[..i],
        // A plain file may still be writing this line
        None if ended && !compressed => return Ok(None),
        None => data,
    };
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let digest = Sha256::digest(line);
    Ok(Some(digest[..16].iter().map(|b| format!("{b:02x}")).collect()))
}

/// What to do with one file in a round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePlan {
    pub size: u64,
    pub mtime_ns: i64,
    pub compressed: bool,
    /// `None` while the file has no complete first line.
    pub fingerprint: Option<String>,
    /// Offset of the first line to read.
    pub start: u64,
    /// Documents of this content from this offset on are removed before the
    /// read, because an earlier read may have added them.
    pub replace_from: Option<u64>,
    /// Nothing to read, the content is indexed up to its end.
    pub skip: bool,
}

/// Options of a plan.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlanOptions {
    /// Read from the start and ignore the stored state.
    pub force: bool,
    /// An earlier bulk import stopped halfway, so documents past the stored
    /// positions may exist.
    pub dirty: bool,
}

/// Decides where a file is read from. `high_water` is the furthest position any
/// read of the same content reached, from the state and from this round.
pub fn plan_file(
    path: &Path,
    snap: &Snapshot,
    high_water: impl Fn(&str) -> u64,
    opts: PlanOptions,
) -> io::Result<FilePlan> {
    let mut file = File::open(path)?;
    let meta = file.metadata()?;
    let size = meta.len();
    let compressed = is_gzip(&mut file, path);
    let mut plan = FilePlan {
        size,
        mtime_ns: mtime_ns(&meta),
        compressed,
        fingerprint: None,
        start: 0,
        replace_from: None,
        skip: false,
    };
    drop(file);

    let head = open_content(path, size, compressed, 0)?;
    let Some(fingerprint) = fingerprint_of(head, compressed)? else {
        // No complete first line yet, whatever was stored no longer applies
        plan.skip = true;
        return Ok(plan);
    };

    let key = path.to_string_lossy();
    let current = if opts.force { None } else { snap.row(&key) };
    let mut rewritten = false;
    match current {
        Some(row) if row.fingerprint == fingerprint => {
            plan.start = row.position;
            if !compressed && plan.start > size {
                // Shorter than what was read of it: the file was rewritten
                plan.start = 0;
                rewritten = true;
            }
        }
        _ if !opts.force => {
            if let Some(donor) = snap.inherit(&fingerprint, &key) {
                plan.start = donor.position;
                if !compressed && plan.start > size {
                    plan.start = size;
                }
            }
        }
        _ => {}
    }

    // A compressed copy that holds exactly what was read before needs no
    // decompression, the size of its content is in the trailer.
    if compressed && plan.start > 0 && plan.start < (1 << 32) {
        let mut file = File::open(path)?;
        if u64::from(gzip_content_size(&mut file, size)) == plan.start {
            plan.skip = true;
        }
    }
    if !plan.skip && !opts.force && (rewritten || opts.dirty || plan.start < high_water(&fingerprint)) {
        plan.replace_from = Some(plan.start);
    }
    plan.fingerprint = Some(fingerprint);
    Ok(plan)
}

/// One line of a content.
#[derive(Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    /// Offset of the start of the line.
    pub offset: u64,
}

/// Reads the lines of a content from an offset and tells how far it got.
pub struct LineReader {
    reader: BufReader<Box<dyn Read + Send>>,
    compressed: bool,
    /// Offset of the next line to read.
    offset: u64,
    scratch: Vec<u8>,
}

impl LineReader {
    pub fn new(stream: Box<dyn Read + Send>, compressed: bool, start: u64) -> Self {
        Self { reader: BufReader::with_capacity(256 * 1024, stream), compressed, offset: start, scratch: Vec::new() }
    }

    /// Offset after the last consumed line. An unterminated last line of a
    /// plain file is not consumed, its rest is written later.
    pub fn consumed(&self) -> u64 {
        self.offset
    }

    fn skip_to_newline(&mut self) -> io::Result<u64> {
        let mut skipped = 0u64;
        loop {
            let buf = self.reader.fill_buf()?;
            if buf.is_empty() {
                return Ok(skipped);
            }
            match buf.iter().position(|b| *b == b'\n') {
                Some(i) => {
                    self.reader.consume(i + 1);
                    return Ok(skipped + i as u64 + 1);
                }
                None => {
                    let n = buf.len();
                    self.reader.consume(n);
                    skipped += n as u64;
                }
            }
        }
    }

    /// The next non empty line, `None` at the end of the content.
    pub fn next_line(&mut self) -> io::Result<Option<Line>> {
        loop {
            self.scratch.clear();
            let n = (&mut self.reader).take(MAX_READ_LINE as u64 + 1).read_until(b'\n', &mut self.scratch)?;
            if n == 0 {
                return Ok(None);
            }
            let terminated = self.scratch.last() == Some(&b'\n');
            if !terminated && n > MAX_READ_LINE {
                // Too long to be a log line, skip the rest of it
                let rest = self.skip_to_newline()?;
                self.offset += n as u64 + rest;
                continue;
            }
            if !terminated && !self.compressed {
                return Ok(None);
            }
            let start = self.offset;
            self.offset += n as u64;
            let mut end = self.scratch.len();
            while end > 0 && (self.scratch[end - 1] == b'\n' || self.scratch[end - 1] == b'\r') {
                end -= 1;
            }
            if end == 0 {
                continue;
            }
            let text = String::from_utf8_lossy(&self.scratch[..end]).into_owned();
            return Ok(Some(Line { text, offset: start }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_gz(path: &Path, data: &str) {
        let mut enc = flate2::write::GzEncoder::new(File::create(path).unwrap(), flate2::Compression::fast());
        enc.write_all(data.as_bytes()).unwrap();
        enc.finish().unwrap();
    }

    fn lines(path: &Path, start: u64) -> (Vec<(String, u64)>, u64) {
        let mut file = File::open(path).unwrap();
        let size = file.metadata().unwrap().len();
        let compressed = is_gzip(&mut file, path);
        let mut r = LineReader::new(open_content(path, size, compressed, start).unwrap(), compressed, start);
        let mut out = Vec::new();
        while let Some(l) = r.next_line().unwrap() {
            out.push((l.text, l.offset));
        }
        (out, r.consumed())
    }

    #[test]
    fn fingerprint_needs_a_complete_first_line() {
        assert_eq!(fingerprint_of(&b"abc"[..], false).unwrap(), None);
        assert!(fingerprint_of(&b"abc"[..], true).unwrap().is_some());
        assert_eq!(fingerprint_of(&b"\n\r\n"[..], false).unwrap(), None);
        let a = fingerprint_of(&b"first line\nsecond"[..], false).unwrap();
        let b = fingerprint_of(&b"\nfirst line\r\nother"[..], false).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, fingerprint_of(&b"another\n"[..], false).unwrap());
        assert_eq!(a.unwrap().len(), 32);
    }

    #[test]
    fn plain_lines_carry_their_offsets_and_a_partial_line_waits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        std::fs::write(&path, "one\n\ntwo\r\nthree").unwrap();
        let (got, consumed) = lines(&path, 0);
        assert_eq!(got, [("one".to_owned(), 0), ("two".to_owned(), 5)]);
        assert_eq!(consumed, 10);
        // The rest of the line arrives
        std::fs::write(&path, "one\n\ntwo\r\nthree\nfour\n").unwrap();
        let (got, consumed) = lines(&path, consumed);
        assert_eq!(got, [("three".to_owned(), 10), ("four".to_owned(), 16)]);
        assert_eq!(consumed, 21);
    }

    #[test]
    fn gzip_content_has_the_same_offsets_as_the_plain_file() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("a.log");
        let gz = dir.path().join("a.log.1.gz");
        let text = "alpha\nbeta\ngamma\ndelta";
        std::fs::write(&plain, text).unwrap();
        write_gz(&gz, text);
        let (from_gz, consumed) = lines(&gz, 0);
        // A compressed file consumes its unterminated last line
        assert_eq!(from_gz.last().unwrap(), &("delta".to_owned(), 17));
        assert_eq!(consumed, text.len() as u64);
        let (tail, _) = lines(&gz, 11);
        assert_eq!(tail, [("gamma".to_owned(), 11), ("delta".to_owned(), 17)]);
        let gz_size = std::fs::metadata(&gz).unwrap().len();
        assert_eq!(content_size_estimate(&gz, gz_size), (text.len() as u64).max(gz_size));
    }

    #[test]
    fn a_huge_line_is_skipped_and_reading_goes_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        let mut data = String::from("ok\n");
        data.push_str(&"x".repeat(MAX_READ_LINE + 10));
        data.push_str("\nafter\n");
        std::fs::write(&path, &data).unwrap();
        let (got, consumed) = lines(&path, 0);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].0, "after");
        assert_eq!(consumed, data.len() as u64);
    }

    #[test]
    fn needs_sync_compares_size_and_mtime() {
        let row = FileRow { size: 10, mtime_ns: 5, ..Default::default() };
        assert!(needs_sync(10, 5, None));
        assert!(!needs_sync(10, 5, Some(&row)));
        assert!(needs_sync(11, 5, Some(&row)));
        assert!(needs_sync(10, 6, Some(&row)));
    }
}
