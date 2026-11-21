//! Text that needs escaping crosses both wire generations byte for byte as the reference worker host encodes it, and
//! decodes back unchanged: every control character, quotes, backslashes, solidus, non-ASCII, astral and separator
//! characters, a byte-order mark and the empty string.

use crate::escape_golden::ESCAPE_FRAMES;
use crate::protocol::{Reply, compile_frame, read_completion, reply_frame};
use crate::record_protocol::{CompilerFault, Prepare, RecordMessage, decode_record_message, record_frame};

const JSON_GEN: &str = "0123abcd";
const CW3_GEN: &str = "cw3-0123456789abcdef";
const CORPUS: [&str; 14] = [
    "000102030405060708090a0b0c0d0e0f",
    "101112131415161718191a1b1c1d1e1f",
    "7f",
    "5c",
    "22",
    "2f",
    "c3a9",
    "f09f9880",
    "e280a8e280a9",
    "efbbbf",
    "3c3e2627",
    "092f705c6e0d0a22",
    "",
    "c2a0c2adcd8fe2808b",
];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Result<Vec<u8>, String> {
    (0..text.len())
        .step_by(2)
        .map(|start| {
            text.get(start..start + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| format!("invalid hex at {start}"))
        })
        .collect()
}

fn corpus_text(encoded: &str) -> Result<String, String> {
    String::from_utf8(unhex(encoded)?).map_err(|error| error.to_string())
}

fn prepare(text: &str) -> RecordMessage {
    RecordMessage::Prepare(Prepare {
        stage: text.to_owned(),
        snapshot: text.to_owned(),
        demand_root: text.to_owned(),
        producer_profile: text.to_owned(),
    })
}

fn fault(text: &str) -> RecordMessage {
    RecordMessage::CompilerFault(CompilerFault {
        fault_root: text.to_owned(),
    })
}

fn encoded_frames() -> Result<Vec<(String, String)>, String> {
    let mut frames = Vec::new();
    for (index, encoded) in CORPUS.iter().enumerate() {
        let text = corpus_text(encoded)?;
        for generation in [JSON_GEN, CW3_GEN] {
            let label = |kind: &str| format!("{index}\t{generation}\t{kind}");
            let arguments = [text.clone(), "x".to_owned()];
            let compile = compile_frame(generation, &arguments, &text, &text, &text, false)?;
            frames.push((label("compile"), hex(&compile)));
            let refused = reply_frame(&Reply::Refused(text.clone()), false, generation)?;
            frames.push((label("refused"), hex(&refused)));
            frames.push((label("prepare"), hex(&record_frame(&prepare(&text), generation)?)));
            frames.push((label("fault"), hex(&record_frame(&fault(&text), generation)?)));
        }
    }
    Ok(frames)
}

#[test]
fn escaped_text_is_byte_identical_to_the_reference() -> Result<(), String> {
    let frames = encoded_frames()?;
    assert_eq!(frames.len(), ESCAPE_FRAMES.len());
    for ((label, encoded), (expected_label, expected)) in frames.iter().zip(ESCAPE_FRAMES) {
        assert_eq!(label, expected_label);
        assert_eq!(encoded, expected, "{label}");
    }
    Ok(())
}

#[test]
fn reference_frames_with_escaped_text_decode_back_unchanged() -> Result<(), String> {
    for (label, frame) in ESCAPE_FRAMES {
        let mut parts = label.split('\t');
        let (Some(index), Some(generation), Some(kind)) = (parts.next(), parts.next(), parts.next()) else {
            return Err(format!("invalid label {label}"));
        };
        let position: usize = index.parse().map_err(|_| format!("invalid index in {label}"))?;
        let text = corpus_text(
            CORPUS
                .get(position)
                .ok_or_else(|| format!("no corpus entry {position}"))?,
        )?;
        let bytes = unhex(frame)?;
        match kind {
            "prepare" => assert_eq!(decode_record_message(&bytes, generation)?, prepare(&text), "{label}"),
            "fault" => assert_eq!(decode_record_message(&bytes, generation)?, fault(&text), "{label}"),
            // An empty reason is no refusal: the reply completes with the refused exit status.
            "refused" if generation == JSON_GEN && text.is_empty() => {
                assert_eq!(read_completion(&bytes, generation)?.status, 1, "{label}")
            }
            "refused" if generation == JSON_GEN => {
                assert_eq!(read_completion(&bytes, generation).err(), Some(text.clone()), "{label}")
            }
            _ => {}
        }
    }
    Ok(())
}
