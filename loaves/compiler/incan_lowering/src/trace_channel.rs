//! The lowering's side of the one tracing channel.
//!
//! With `INCAN_ENGINE_TRACE` naming a directory, a record goes to the compiler process's one file there,
//! `engine-compiler-<pid>.jsonl`, with an `operation` naming its category. Tracing never writes the console.
//! Otherwise, for one more release, a category's former variable selects its former destination: standard output
//! for `lowering` and `lowering_footprint`, the file `OVEN_TRACE` names for `redo`.

use std::io::Write;

const CHANNEL: &str = "INCAN_ENGINE_TRACE";

/// Whether a category is traced: the channel is on, or the category's former variable is set.
pub fn traced(former: &str) -> bool {
    !channel().is_empty() || std::env::var_os(former).is_some()
}

/// Trace one line of `category`: into the channel when it is on, otherwise where `former` sent it.
pub fn emit(category: &str, former: &str, text: &str) {
    let directory = channel();
    if !directory.is_empty() {
        if let Ok(key) = serde_json::to_string(text) {
            let process = std::process::id();
            let head = "{\"schema\":\"incan.engine.trace/1\",\"kind\":\"line\",\"producer\":\"compiler\"";
            let record = format!("{head},\"operation\":\"{category}\",\"key\":{key},\"process\":{process}}}\n");
            append(&format!("{directory}/engine-compiler-{process}.jsonl"), &record);
        }
        return;
    }
    if former == "OVEN_TRACE" {
        if let Ok(destination) = std::env::var(former)
            && !destination.is_empty()
            && destination != "-"
        {
            append(&destination, &format!("{text}\n"));
        }
        return;
    }
    if std::env::var_os(former).is_some() {
        println!("{text}");
    }
}

/// Trace whether one keyed frame was restored from its product or lowered, into the channel only.
///
/// `outcome` is `reused`, `lowered`, `unkept` (lowered without a product) or `admitted` (restored from its module's
/// plan page); `kind` is `ordinary` or `instance`. A frame without a product key or page is not traced.
pub fn frame_product(outcome: &str, kind: &str, name: &str) {
    if !channel().is_empty() {
        emit("lowering_product", "", &format!("{outcome} {kind} {name}"));
    }
}

fn channel() -> String {
    std::env::var(CHANNEL).unwrap_or_default()
}

/// Append one whole line; a sink that cannot be written loses the line, never the compilation.
fn append(destination: &str, text: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(destination) {
        let _ = file.write_all(text.as_bytes());
    }
}
