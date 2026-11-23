//! Export the native symbols a shared carrier links in from its crates' bundled static libraries.
//!
//! Rustc exports a foreign item from a dylib only when the item's `extern` block names a static library with
//! `#[link]`. A crate whose build links its bundled library by a command-line `-l static=` (as `zstd-sys` does)
//! leaves its foreign items unattributed, so the carrier neither keeps nor exports them, and a generic function
//! instantiated in the consumer cannot reach them. The carrier therefore exports every foreign item of a statically
//! linked crate that a statically linked rlib's archive index defines.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use rustc_hir::def::DefKind;
use rustc_hir::def_id::{CrateNum, LOCAL_CRATE};
use rustc_middle::middle::dependency_format::Linkage;
use rustc_middle::middle::exported_symbols::{ExportedSymbol, SymbolExportInfo, SymbolExportKind, SymbolExportLevel};
use rustc_middle::query::LocalCrate;
use rustc_middle::ty::{Instance, SymbolName, TyCtxt};

/// The member-header size of an `ar` archive, and where the member's decimal size is in it.
const HEADER: usize = 60;
const SIZE_FIELD: std::ops::Range<usize> = 48..58;

/// Replace the carrier's exported non-generic symbols with rustc's own plus the bundled native ones.
pub(crate) fn provide(providers: &mut rustc_middle::util::Providers) {
    providers.queries.exported_non_generic_symbols = exported_with_bundled_natives;
}

/// Rustc's exported symbols for the carrier, and each bundled native symbol as a used C-level export: `used` puts
/// it in the link's symbol object, which loads its archive member and keeps it from dead stripping.
fn exported_with_bundled_natives<'tcx>(
    tcx: TyCtxt<'tcx>,
    key: LocalCrate,
) -> &'tcx [(ExportedSymbol<'tcx>, SymbolExportInfo)] {
    let default = (rustc_interface::DEFAULT_QUERY_PROVIDERS
        .queries
        .exported_non_generic_symbols)(tcx, key);
    let natives = bundled_natives(tcx);
    if natives.is_empty() {
        return default;
    }
    let mut symbols = default.to_vec();
    for (name, kind) in natives {
        let info = SymbolExportInfo {
            level: SymbolExportLevel::C,
            kind,
            used: true,
            rustc_std_internal_symbol: false,
        };
        symbols.push((ExportedSymbol::NoDefId(SymbolName::new(tcx, &name)), info));
    }
    tcx.arena.alloc_from_iter(symbols)
}

/// The foreign items of statically linked crates that a statically linked crate's bundled library defines, by link
/// name. A symbol one of those crates already exports as a Rust definition is left to rustc.
fn bundled_natives(tcx: TyCtxt<'_>) -> Vec<(String, SymbolExportKind)> {
    let formats = tcx.dependency_formats(());
    let Some(linkage) = formats.values().next() else {
        return Vec::new();
    };
    let linked = linkage
        .iter_enumerated()
        .filter(|(number, format)| **format == Linkage::Static && *number != LOCAL_CRATE)
        .map(|(number, _)| number)
        .collect::<Vec<CrateNum>>();

    // ---- Symbols the bundled libraries define ----
    let mut defined = BTreeSet::new();
    let mut exported = BTreeSet::new();
    for number in &linked {
        let bundles = tcx
            .native_libraries(*number)
            .iter()
            .any(|library| library.kind.is_statically_included());
        let Some(archive) = tcx.used_crate_source(*number).rlib.as_ref().filter(|_| bundles) else {
            continue;
        };
        match archive_symbols(archive) {
            Ok(names) => defined.extend(names),
            Err(error) => tcx.dcx().fatal(format!(
                "cannot read the archive index of {}: {error}",
                archive.display()
            )),
        }
        for (symbol, _) in tcx.exported_non_generic_symbols(*number) {
            if let ExportedSymbol::NonGeneric(definition) = symbol {
                exported.insert(tcx.symbol_name(Instance::mono(tcx, *definition)).name.to_string());
            }
        }
    }
    if defined.is_empty() {
        return Vec::new();
    }

    // ---- Foreign items that name one of them ----
    let prefix = if tcx.sess.target.is_like_darwin { "_" } else { "" };
    let mut natives = Vec::new();
    let mut seen = BTreeSet::new();
    for number in &linked {
        for module in tcx.foreign_modules(*number).values() {
            for item in &module.foreign_items {
                let kind = match tcx.def_kind(*item) {
                    DefKind::Fn => SymbolExportKind::Text,
                    DefKind::Static { .. } => SymbolExportKind::Data,
                    _ => continue,
                };
                let name = tcx.symbol_name(Instance::mono(tcx, *item)).name.to_string();
                if defined.contains(&format!("{prefix}{name}"))
                    && !exported.contains(&name)
                    && seen.insert(name.clone())
                {
                    natives.push((name, kind));
                }
            }
        }
    }
    natives.sort_by(|left, right| left.0.cmp(&right.0));
    natives
}

/// The global symbols an archive's index names, in the BSD (`__.SYMDEF`) or GNU (`/`) form, 32-bit or 64-bit. An
/// archive without an index names none.
pub(crate) fn archive_symbols(path: &Path) -> std::io::Result<BTreeSet<String>> {
    let mut file = File::open(path)?;
    let mut start = [0u8; 8 + HEADER];
    if file.read_exact(&mut start).is_err() || &start[..8] != b"!<arch>\n" {
        return Ok(BTreeSet::new());
    }
    let header = &start[8..];
    let size = decimal(&header[SIZE_FIELD]);
    let mut name = String::from_utf8_lossy(&header[..16]).trim_end().to_string();
    let mut data = vec![0u8; size];
    file.read_exact(&mut data)?;
    // A BSD long name is stored in front of the member's data.
    let mut body = data.as_slice();
    if let Some(length) = name.strip_prefix("#1/").map(|digits| decimal(digits.as_bytes())) {
        if length > body.len() {
            return Ok(BTreeSet::new());
        }
        name = String::from_utf8_lossy(&body[..length])
            .trim_end_matches('\0')
            .to_string();
        body = &body[length..];
    }
    Ok(match name.as_str() {
        "__.SYMDEF" | "__.SYMDEF SORTED" => bsd_index(body, 4),
        "__.SYMDEF_64" | "__.SYMDEF_64 SORTED" => bsd_index(body, 8),
        "/" => gnu_index(body, 4),
        "/SYM64/" => gnu_index(body, 8),
        _ => BTreeSet::new(),
    })
}

/// A header's space-padded decimal field; a malformed field reads as zero.
fn decimal(field: &[u8]) -> usize {
    String::from_utf8_lossy(field).trim().parse().unwrap_or(0)
}

/// One little-endian (BSD) or big-endian (GNU) unsigned field of `width` bytes at `offset`.
fn field(data: &[u8], offset: usize, width: usize, little: bool) -> Option<usize> {
    let bytes = data.get(offset..offset.checked_add(width)?)?;
    let mut value = 0usize;
    for index in 0..width {
        let byte = if little { bytes[width - 1 - index] } else { bytes[index] };
        value = value.checked_mul(256)?.checked_add(byte as usize)?;
    }
    Some(value)
}

/// The NUL-terminated name that starts at `offset`.
fn name_at(strings: &[u8], offset: usize) -> Option<String> {
    let rest = strings.get(offset..)?;
    let end = rest.iter().position(|byte| *byte == 0)?;
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// A BSD index: the byte size of its `(name offset, member offset)` pairs, the pairs, the string table's size, and
/// the string table.
fn bsd_index(data: &[u8], width: usize) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let Some(pairs) = field(data, 0, width, true) else {
        return names;
    };
    let strings = width + pairs + width;
    for entry in 0..pairs / (2 * width) {
        let offset = field(data, width + entry * 2 * width, width, true);
        if let Some(name) = offset.and_then(|offset| name_at(data.get(strings..).unwrap_or(&[]), offset)) {
            names.insert(name);
        }
    }
    names
}

/// A GNU index: a count, that many member offsets, then that many NUL-terminated names.
fn gnu_index(data: &[u8], width: usize) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let Some(count) = field(data, 0, width, false) else {
        return names;
    };
    let mut offset = width.saturating_add(count.saturating_mul(width));
    for _ in 0..count {
        let Some(name) = name_at(data, offset) else {
            break;
        };
        offset += name.len() + 1;
        names.insert(name);
    }
    names
}
