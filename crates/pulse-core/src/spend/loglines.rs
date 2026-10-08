// Ported from upstream Sources/Pulse/Usage/LogLines.swift.
//! Streaming line IO. Buffers hold one chunk plus any cross-chunk line, never the file; a line
//! inside a chunk is handed out as a slice of it. Lines are raw bytes: a trailing `\r`, invalid
//! UTF-8 and NUL are kept as written, and empty lines are skipped.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// The default read size: 64 KiB.
pub const CHUNK_SIZE: usize = 64 * 1024;

/// A lending line reader over any byte source.
pub struct LineReader<R> {
    reader: R,
    chunk: Vec<u8>,
    pos: usize,
    end: usize,
    carry: Vec<u8>,
    done: bool,
}

enum Found {
    Chunk(usize, usize),
    Carry,
    End,
}

impl<R: Read> LineReader<R> {
    pub fn new(reader: R) -> Self {
        Self::with_chunk_size(reader, CHUNK_SIZE)
    }

    pub fn with_chunk_size(reader: R, chunk_size: usize) -> Self {
        Self { reader, chunk: vec![0; chunk_size.max(1)], pos: 0, end: 0, carry: Vec::new(), done: false }
    }

    /// The next non-empty line without its `\n`, or None at the end. A read error ends the
    /// stream, like an unreadable file contributes nothing further.
    pub fn next_line(&mut self) -> Option<&[u8]> {
        self.carry.clear();
        let found = loop {
            if self.pos < self.end {
                match memchr::memchr(b'\n', &self.chunk[self.pos..self.end]) {
                    Some(offset) => {
                        let (start, stop) = (self.pos, self.pos + offset);
                        self.pos = stop + 1;
                        if self.carry.is_empty() {
                            if start == stop {
                                continue;
                            }
                            break Found::Chunk(start, stop);
                        }
                        self.carry.extend_from_slice(&self.chunk[start..stop]);
                        break Found::Carry;
                    }
                    None => {
                        self.carry.extend_from_slice(&self.chunk[self.pos..self.end]);
                        self.pos = self.end;
                    }
                }
            }
            if self.done {
                break if self.carry.is_empty() { Found::End } else { Found::Carry };
            }
            match self.reader.read(&mut self.chunk) {
                Ok(0) | Err(_) => self.done = true,
                Ok(n) => {
                    self.pos = 0;
                    self.end = n;
                }
            }
        };
        match found {
            Found::Chunk(start, stop) => Some(&self.chunk[start..stop]),
            Found::Carry => Some(&self.carry),
            Found::End => None,
        }
    }
}

/// Every line of a file, in order. An unreadable file has no lines.
pub fn for_each_line(path: &Path, mut body: impl FnMut(&[u8])) {
    let Ok(file) = File::open(path) else { return };
    let mut lines = LineReader::new(file);
    while let Some(line) = lines.next_line() {
        body(line);
    }
}

/// Every line of an in-memory buffer, with the same semantics as a disk read.
pub fn for_each_line_in(data: &[u8], mut body: impl FnMut(&[u8])) {
    let mut lines = LineReader::new(data);
    while let Some(line) = lines.next_line() {
        body(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect<R: Read>(mut lines: LineReader<R>) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(line) = lines.next_line() {
            out.push(line.to_vec());
        }
        out
    }

    #[test]
    fn disk_chunks_preserve_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bytes.log");
        let expected: Vec<Vec<u8>> = vec![
            b"first\r".to_vec(),
            vec![0x0d],
            vec![0, 0xff, 0x80],
            "数🙂".repeat(20_000).into_bytes(),
            b"last".to_vec(),
        ];
        let mut input = vec![0x0a, 0x0a];
        for (index, line) in expected.iter().enumerate() {
            input.extend_from_slice(line);
            if index < expected.len() - 1 {
                input.extend_from_slice(&[0x0a, 0x0a]);
            }
        }
        std::fs::write(&file, &input).unwrap();
        for chunk in [1, 2, 7, 64, 65_536] {
            let lines = collect(LineReader::with_chunk_size(File::open(&file).unwrap(), chunk));
            assert_eq!(lines, expected, "chunk size {chunk}");
        }
    }

    #[test]
    fn eof_and_blank_lines_match_disk_semantics() {
        for suffix in ["", "\n", "\n\n"] {
            let input = format!("alpha\n\nbeta{suffix}");
            let lines = collect(LineReader::new(input.as_bytes()));
            assert_eq!(lines, vec![b"alpha".to_vec(), b"beta".to_vec()]);
        }
        for input in [&b""[..], &b"\n"[..], &b"\n\n"[..]] {
            assert!(collect(LineReader::new(input)).is_empty());
        }
    }

    #[test]
    fn readers_have_independent_positions() {
        let data = b"one\ntwo\nthree";
        let mut first = LineReader::new(&data[..]);
        let mut second = LineReader::new(&data[..]);
        let retained = first.next_line().unwrap().to_vec();
        assert_eq!(first.next_line().unwrap(), b"two");
        assert_eq!(second.next_line().unwrap(), retained.as_slice());
        assert_eq!(first.next_line().unwrap(), b"three");
        assert!(first.next_line().is_none());
        assert_eq!(second.next_line().unwrap(), b"two");
        assert_eq!(retained, b"one");
    }
}
