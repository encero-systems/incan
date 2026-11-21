//! A corpus recording keeps every input and reference answer and replaces only the outputs a replay wrote, and is
//! refused when any call's answer differs from its reference.

use std::path::PathBuf;

use crate::corpus::{Case, Corpus};
use crate::corpus_recording::{Recording, recordable};

/// A fresh scratch directory for one test.
fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("corpus-recording-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path
}

/// Any call answer that differs from its reference refuses the whole recording, naming the first difference.
#[test]
fn a_recording_is_refused_when_any_answer_differs() {
    assert!(recordable(&[]).is_ok());
    let refused = recordable(&["case step 2 (unit): expected Complete; got Refused".to_owned()]);
    assert!(refused.is_err_and(|reason| reason.contains("1 call answers differ") && reason.contains("step 2")));
}

/// A recording copies the source blobs, stores each output file once by digest and writes cases and publications a
/// corpus reads back; an existing destination is refused rather than overwritten.
#[test]
fn a_recording_reads_back_as_a_corpus() -> Result<(), String> {
    let source = scratch("source");
    std::fs::create_dir_all(source.join("blobs")).map_err(|error| error.to_string())?;
    std::fs::write(source.join("blobs").join("input"), b"module bytes").map_err(|error| error.to_string())?;
    let root = scratch("recorded");
    let recording = Recording::create(&source, &root)?;
    assert!(
        Recording::create(&source, &root).is_err(),
        "a recording overwrote an existing destination"
    );
    let outputs = scratch("outputs");
    std::fs::create_dir_all(outputs.join("nested")).map_err(|error| error.to_string())?;
    std::fs::write(outputs.join("b.incnfunction"), b"second").map_err(|error| error.to_string())?;
    std::fs::write(outputs.join("a.event"), b"first").map_err(|error| error.to_string())?;
    let files = recording.files(&outputs)?;
    assert!(
        files
            .iter()
            .map(|(name, _)| name.as_str())
            .eq(["a.event", "b.incnfunction"])
    );
    let case = Case {
        schema: "recorded".to_owned(),
        ..Case::default()
    };
    recording.write_case("one.case", &case)?;
    recording.write_publication("plan", &[("a.event".to_owned(), b"first".to_vec())])?;
    let corpus = Corpus::open(&root);
    assert!(corpus.blob("input")? == b"module bytes");
    assert!(corpus.blob(&files[1].1)? == b"second");
    assert!(corpus.case(&root.join("cases").join("one.case"))?.schema == "recorded");
    assert!(corpus.publication("plan")? == Some(vec![("a.event".to_owned(), b"first".to_vec())]));
    for path in [source, root, outputs] {
        let _ = std::fs::remove_dir_all(path);
    }
    Ok(())
}
