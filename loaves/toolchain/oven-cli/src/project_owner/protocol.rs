//! Bounded, request-local messages for the shared project owner.
//!
//! The transport carries intent and results, never cached compiler authority. A generation mismatch refuses the
//! request before dispatch. Progress is framed separately so machine-readable reports remain intact.

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Bound both requests and reports before allocating an incoming frame.
const MAX_FRAME: usize = 16 * 1024 * 1024;

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

/// Read one complete bounded frame; truncated and oversized input never reaches dispatch.
pub(super) fn read<T: for<'de> Deserialize<'de>>(reader: &mut impl Read) -> io::Result<T> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let length = usize::try_from(u32::from_be_bytes(header)).map_err(io::Error::other)?;
    if length > MAX_FRAME {
        return Err(io::Error::other("project-owner frame exceeds size limit"));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

/// Write and flush one bounded message, preserving ordering between progress and its terminal answer.
pub(super) fn write(writer: &mut impl Write, message: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::other("project-owner frame exceeds size limit"));
    }
    let length = u32::try_from(bytes.len()).map_err(io::Error::other)?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&bytes)?;
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
