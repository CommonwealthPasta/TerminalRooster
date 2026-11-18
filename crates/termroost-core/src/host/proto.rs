//! What TerminalRooster and a terminal's helper process say to each other.
//!
//! Over a loopback TCP connection, as frames: one byte saying what the frame
//! is, the length of its payload (u32, little endian), then the payload.
//! Control messages carry JSON; terminal input and output go as they are.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::PaneId;
use crate::session::GridSize;

/// Changes whenever a message changes. A helper only talks to clients of its
/// own version (a helper outlives the TerminalRooster that started it, and
/// may meet a newer one).
pub(crate) const VERSION: u32 = 1;

/// Larger frames are taken for garbage rather than allocated.
const MAX_FRAME: usize = 64 << 20;

// From the client.
/// [`Hello`]: the first frame of every connection.
pub(crate) const HELLO: u8 = 1;
/// Bytes for the program, as typed.
pub(crate) const INPUT: u8 = 2;
/// A [`GridSize`]: the client's view changed size.
pub(crate) const RESIZE: u8 = 3;
/// End the program and the helper (the terminal was closed).
pub(crate) const END: u8 = 4;
/// A paused client shows the terminal again; the one showing it steps aside.
pub(crate) const TAKE: u8 = 5;

// From the helper.
/// [`Welcome`]: the client shows the terminal. An `OUTPUT` frame with the
/// whole screen follows, then the program's output as it comes. Comes
/// again after `PAUSED`, when the client's turn comes back.
pub(crate) const WELCOME: u8 = 0x81;
/// Bytes the program wrote.
pub(crate) const OUTPUT: u8 = 0x82;
/// [`Exited`]: the program ended; the helper ends too once it has said so.
pub(crate) const EXITED: u8 = 0x83;
/// Another client attached and took over; the connection closes.
pub(crate) const KICKED: u8 = 0x84;
/// [`Refusal`]: the client was not attached; the connection closes.
pub(crate) const REFUSED: u8 = 0x85;
/// Another client shows the terminal now. This one stays connected, without
/// output, until it leaves, sends `TAKE`, or gets a `WELCOME` (its turn came
/// back as the other client left).
pub(crate) const PAUSED: u8 = 0x86;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Hello {
    pub token: String,
    pub version: u32,
    /// Take over from a client showing the terminal, which then pauses (or
    /// gets `KICKED`, if it doesn't stay). Without it, such a client means
    /// waiting (`PAUSED`), or `Refusal::Busy` for a client that doesn't stay.
    pub steal: bool,
    /// Stay connected, paused, while another client shows the terminal.
    #[serde(default)]
    pub stay: bool,
    pub size: GridSize,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Welcome {
    pub version: u32,
    pub shell_pid: u32,
    pub title: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Exited {
    pub code: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Refusal {
    Token,
    Version,
    /// Another client is attached.
    Busy,
}

/// How to reach a running helper. It writes this to [`conn_file`] once it
/// listens and removes it when it ends.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ConnInfo {
    pub version: u32,
    /// The helper's process id.
    pub pid: u32,
    pub port: u16,
    /// What a client must send in its [`Hello`]: the file is readable by
    /// its user only, the port by anyone on the machine.
    pub token: String,
}

/// What a new helper runs, read from its standard input.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Spec {
    pub pane: PaneId,
    /// Where the helper writes its [`ConnInfo`].
    pub conn_file: PathBuf,
    pub token: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub size: GridSize,
    pub scrollback_lines: usize,
    /// Output to show above the program's: a restored terminal's scrollback.
    pub preload: Option<String>,
}

/// Where the helpers of a data directory keep their [`ConnInfo`]s.
pub(crate) fn hosts_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("hosts")
}

/// Where the helper of a pane keeps its [`ConnInfo`].
pub(crate) fn conn_file(data_dir: &Path, pane: PaneId) -> PathBuf {
    hosts_dir(data_dir).join(format!("{}.json", pane.0))
}

pub(crate) fn read_conn(path: &Path) -> Option<ConnInfo> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Writes the file whole or not at all, so a reader never sees half of it.
pub(crate) fn write_conn(path: &Path, info: &ConnInfo) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(info)?)?;
    std::fs::rename(&tmp, path)
}

/// Removes this helper's [`ConnInfo`], unless the file is another's by now:
/// the helper that took over its terminal after a restart.
pub(crate) fn remove_conn_if_mine(path: &Path) {
    remove_conn_of(path, std::process::id());
}

/// Removes a [`ConnInfo`] if it is still that of helper `pid`.
pub(crate) fn remove_conn_of(path: &Path, pid: u32) {
    if read_conn(path).is_some_and(|info| info.pid == pid) {
        let _ = std::fs::remove_file(path);
    }
}

/// A secret for a new helper's clients to show.
pub(crate) fn new_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Compares in time independent of where the strings differ.
pub(crate) fn token_matches(given: &str, expected: &str) -> bool {
    given.len() == expected.len() && given.bytes().zip(expected.bytes()).fold(0, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// Writes one frame (in one write, so it goes out whole).
pub(crate) fn write_frame(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.push(kind);
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf)
}

/// Writes a frame with a JSON payload.
pub(crate) fn write_json(w: &mut impl Write, kind: u8, msg: &impl Serialize) -> io::Result<()> {
    write_frame(w, kind, &serde_json::to_vec(msg)?)
}

pub(crate) fn parse<'a, T: Deserialize<'a>>(payload: &'a [u8]) -> io::Result<T> {
    serde_json::from_slice(payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub kind: u8,
    pub payload: Vec<u8>,
}

/// Reads frames from a stream. A read timeout set on the stream returns from
/// [`FrameReader::next`] with the error, keeping what was read of a frame.
pub(crate) struct FrameReader<R> {
    inner: R,
    buf: Vec<u8>,
    /// Where the unread bytes of `buf` start.
    start: usize,
    chunk: Box<[u8]>,
}

impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self { inner, buf: Vec::new(), start: 0, chunk: vec![0; 64 * 1024].into_boxed_slice() }
    }

    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    /// The next frame, or `None` where the stream ends between frames.
    pub fn next(&mut self) -> io::Result<Option<Frame>> {
        loop {
            if let Some(frame) = self.take()? {
                return Ok(Some(frame));
            }
            let n = self.inner.read(&mut self.chunk)?;
            if n == 0 {
                return if self.start == self.buf.len() { Ok(None) } else { Err(io::ErrorKind::UnexpectedEof.into()) };
            }
            // Drop what was read before appending, so `buf` doesn't grow.
            if self.start > 0 {
                self.buf.drain(..self.start);
                self.start = 0;
            }
            self.buf.extend_from_slice(&self.chunk[..n]);
        }
    }

    fn take(&mut self) -> io::Result<Option<Frame>> {
        let rest = &self.buf[self.start..];
        if rest.len() < 5 {
            return Ok(None);
        }
        let len = u32::from_le_bytes([rest[1], rest[2], rest[3], rest[4]]) as usize;
        if len > MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
        }
        if rest.len() < 5 + len {
            return Ok(None);
        }
        let frame = Frame { kind: rest[0], payload: rest[5..5 + len].to_vec() };
        self.start += 5 + len;
        Ok(Some(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hands out its bytes a few at a time, and times out once in between.
    struct Trickle {
        data: Vec<u8>,
        pos: usize,
        timed_out: bool,
    }

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            // Inside the first frame (5 bytes of header, 5 of payload).
            if self.pos == 6 && !self.timed_out {
                self.timed_out = true;
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let n = 3.min(buf.len()).min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn frames_survive_short_reads_and_timeouts() {
        let mut data = Vec::new();
        write_frame(&mut data, OUTPUT, b"hello").unwrap();
        write_frame(&mut data, KICKED, b"").unwrap();
        write_json(&mut data, EXITED, &Exited { code: Some(3) }).unwrap();
        let mut r = FrameReader::new(Trickle { data, pos: 0, timed_out: false });

        let err = r.next().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(r.next().unwrap().unwrap(), Frame { kind: OUTPUT, payload: b"hello".to_vec() });
        assert_eq!(r.next().unwrap().unwrap(), Frame { kind: KICKED, payload: Vec::new() });
        let exited = r.next().unwrap().unwrap();
        assert_eq!(exited.kind, EXITED);
        assert_eq!(parse::<Exited>(&exited.payload).unwrap().code, Some(3));
        assert_eq!(r.next().unwrap(), None);
    }

    #[test]
    fn a_stream_cut_inside_a_frame_is_an_error() {
        let mut data = Vec::new();
        write_frame(&mut data, OUTPUT, b"hello").unwrap();
        data.truncate(7);
        let mut r = FrameReader::new(&data[..]);
        assert_eq!(r.next().unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn oversized_frames_are_refused() {
        let mut data = vec![OUTPUT];
        data.extend_from_slice(&u32::MAX.to_le_bytes());
        let mut r = FrameReader::new(&data[..]);
        assert_eq!(r.next().unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn tokens() {
        let t = new_token();
        assert_eq!(t.len(), 32);
        assert_ne!(t, new_token());
        assert!(token_matches(&t, &t.clone()));
        assert!(!token_matches(&t, &t[..31]));
        assert!(!token_matches("abc", "abd"));
    }
}
