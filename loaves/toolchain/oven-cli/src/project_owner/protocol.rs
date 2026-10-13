//! Bounded wire fragments for request-local messages for the shared project owner.
//!
//! The transport carries intent and results, never cached compiler authority. A generation mismatch refuses the
//! request before dispatch. Progress is framed separately so machine-readable reports remain intact.

use serde::{Deserialize, Serialize};
use std::io::{self, BufReader, Cursor, Read, Write};

/// Bound each wire fragment before allocation; source text and reports may span multiple fragments.
const MAX_FRAME: usize = 16 * 1024 * 1024;
/// Continuation is explicit, so the decoder never consumes the next independent reply.
const MORE_FRAGMENTS: u32 = 1 << 31;

/// One command intent addressed to a compatible owner generation.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    /// Protocol and executable/configuration generation selected by the client.
    pub generation: String,
    /// Build or lifecycle intent; the owner validates it before entering the driver.
    pub intent: serde_json::Value,
}

/// A request-local progress message or terminal answer.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Reply {
    /// Human progress, rendered by this request's client only.
    Progress { message: String },
    /// Request-local stderr, kept separate from console and JSON stdout records.
    Diagnostic { message: String },
    /// Completed result, including serving-process identity for lifecycle verification.
    Complete { pid: u32, result: serde_json::Value },
    /// Explicit rejection or compiler failure; never an instruction to run a second route.
    Failure { message: String, exit_code: i32 },
}

/// Read one complete message without letting a declared fragment length allocate beyond the wire bound.
/// Small messages retain the slice decoder. Larger messages decode from a framed stream rather than copying
/// every fragment into a second whole-message buffer. Truncated fragments never reach successful dispatch.
pub(super) fn read<T: for<'de> Deserialize<'de>>(reader: &mut impl Read) -> io::Result<T> {
    let (length, more) = read_header(reader)?;
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    if !more {
        return serde_json::from_slice(&bytes).map_err(io::Error::other);
    }
    let tail = FragmentReader {
        reader,
        remaining: 0,
        more: true,
    };
    serde_json::from_reader(Cursor::new(bytes).chain(BufReader::new(tail))).map_err(io::Error::other)
}

/// Decode a fragment header before allocating or reading its body; empty continuations cannot make progress.
fn read_header(reader: &mut impl Read) -> io::Result<(usize, bool)> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let header = u32::from_be_bytes(header);
    let length = usize::try_from(header & !MORE_FRAGMENTS).map_err(io::Error::other)?;
    let more = header & MORE_FRAGMENTS != 0;
    if length > MAX_FRAME || (more && length == 0) {
        return Err(io::Error::other(
            "project-owner fragment exceeds size limit or makes no progress",
        ));
    }
    Ok((length, more))
}

/// Present exactly one continued message as bytes, leaving the following independent frame unread.
struct FragmentReader<'a, R> {
    reader: &'a mut R,
    remaining: usize,
    more: bool,
}

impl<R: Read> Read for FragmentReader<'_, R> {
    /// Read only within the current bounded fragment and require every promised continuation to arrive.
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 && self.more {
            (self.remaining, self.more) = read_header(self.reader)?;
        }
        if self.remaining == 0 {
            return Ok(0);
        }
        let limit = bytes.len().min(self.remaining);
        let count = self.reader.read(&mut bytes[..limit])?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated project-owner fragment",
            ));
        }
        self.remaining -= count;
        Ok(count)
    }
}

/// Write one message as bounded fragments, preserving ordering between progress and its terminal answer.
/// The common single-fragment representation is unchanged. Continuations allow large source files and reports
/// through the same owner operation without adding a fallback route or raising the per-fragment allocation bound.
pub(super) fn write(writer: &mut impl Write, message: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    let mut fragments = bytes.chunks(MAX_FRAME).peekable();
    while let Some(fragment) = fragments.next() {
        let mut header = u32::try_from(fragment.len()).map_err(io::Error::other)?;
        if fragments.peek().is_some() {
            header |= MORE_FRAGMENTS;
        }
        writer.write_all(&header.to_be_bytes())?;
        writer.write_all(fragment)?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Oversized headers fail without waiting for or allocating their claimed body.
    #[test]
    fn oversized_and_truncated_frames_refuse() -> Result<(), Box<dyn std::error::Error>> {
        let length = u32::try_from(MAX_FRAME + 1)?;
        assert!(read::<Request>(&mut length.to_be_bytes().as_slice()).is_err());
        assert!(read::<Request>(&mut [0, 0, 0, 20, b'{'].as_slice()).is_err());
        Ok(())
    }

    /// Binary framing isolates consecutive messages and preserves Unicode and embedded newlines.
    #[test]
    fn progress_and_result_are_independent_frames() -> Result<(), Box<dyn std::error::Error>> {
        let mut bytes = Vec::new();
        write(
            &mut bytes,
            &Reply::Progress {
                message: "a\nλ".to_string(),
            },
        )?;
        write(
            &mut bytes,
            &Reply::Complete {
                pid: 7,
                result: serde_json::json!({"status":"success"}),
            },
        )?;
        write(
            &mut bytes,
            &Reply::Diagnostic {
                message: "diagnostic\n".to_string(),
            },
        )?;
        let mut reader = bytes.as_slice();
        assert!(matches!(read::<Reply>(&mut reader)?, Reply::Progress { message } if message == "a\nλ"));
        assert!(matches!(read::<Reply>(&mut reader)?, Reply::Complete { pid: 7, .. }));
        assert!(matches!(read::<Reply>(&mut reader)?, Reply::Diagnostic { message } if message == "diagnostic\n"));
        assert!(reader.is_empty());
        Ok(())
    }

    /// A large Unicode/escaped reply spans fragments without consuming the following terminal frame.
    #[test]
    fn large_reply_preserves_unicode_and_next_message() -> Result<(), Box<dyn std::error::Error>> {
        let message = "λ\"\n".repeat(MAX_FRAME / 4 + 1);
        let mut wire = Vec::new();
        write(
            &mut wire,
            &Reply::Progress {
                message: message.clone(),
            },
        )?;
        write(
            &mut wire,
            &Reply::Complete {
                pid: 7,
                result: serde_json::json!(1),
            },
        )?;
        let mut input = wire.as_slice();
        assert!(matches!(read::<Reply>(&mut input)?, Reply::Progress { message: actual } if actual == message));
        assert!(matches!(read::<Reply>(&mut input)?, Reply::Complete { pid: 7, .. }));
        assert!(input.is_empty());
        Ok(())
    }

    /// An initial valid fragment cannot hide missing, empty-continuation or oversized follow-up fragments.
    #[test]
    fn continued_message_requires_valid_complete_fragments() -> Result<(), Box<dyn std::error::Error>> {
        for suffix in [
            Vec::new(),
            MORE_FRAGMENTS.to_be_bytes().to_vec(),
            u32::try_from(MAX_FRAME + 1)?.to_be_bytes().to_vec(),
            vec![0, 0, 0, 3, b'x'],
        ] {
            let mut wire = (MORE_FRAGMENTS | 1).to_be_bytes().to_vec();
            wire.push(b'"');
            wire.extend(suffix);
            assert!(read::<String>(&mut wire.as_slice()).is_err());
        }
        Ok(())
    }

    /// Unknown protocol fields cannot silently alter a command's meaning.
    #[test]
    fn unknown_request_fields_refuse() -> Result<(), Box<dyn std::error::Error>> {
        let mut bytes = Vec::new();
        write(
            &mut bytes,
            &serde_json::json!({"generation":"test", "intent":{}, "unrecognized":true}),
        )?;
        assert!(read::<Request>(&mut bytes.as_slice()).is_err());
        Ok(())
    }
}
