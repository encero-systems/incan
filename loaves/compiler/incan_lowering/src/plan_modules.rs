//! A module's admitted plan record: the page a worker restores its functions from, and the note of what it restored.

mod compact;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use incan_mir_plan_native::{
    ADMITTED_MODULE_NOTE, AdmittedFunction, AdmittedModule, MAX_PLAN_MODULE_SELECTION_BYTES,
    PARTIAL_MODULE_NOTE, PLAN_MODULE_SELECTION_PAGE, POSITIONED_MODULE_PAGE, PlanModuleFunction,
    PlanModuleSelectionRecord, PlanModuleValue, PlanSelectedModule, PlanSelectionScope,
    REPOSITION_MODULE_PAGE, decode_plan_module, decode_plan_module_selection, encode_module_note,
    write_module_placement,
};

use sha2::{Digest, Sha256};

use crate::resident_products::{ResidentPage, resident_full_page, resident_page};

pub use compact::{AuthenticatedWholeAdmission, read_plan_module_admissions};

/// The file of a staged directory naming every page Oven offers by the immutable store frame that holds it instead of
/// a copy, one line per page: its file name, the segment file, the value's offset and length, and its content digest,
/// separated by tabs. Oven writes it once per request.
pub const FRAME_LIST: &str = "frames.list";

/// Where Bakery places the plan record of an admitted module: named by the module, never by a product key. A page
/// offered by its store frame is named in its directory's frame list instead of having a file.
pub fn module_page(products: &str, module: &str) -> PathBuf {
    let basename = hex::encode(module.as_bytes());
    let placed = Path::new(products).join(format!("{basename}{POSITIONED_MODULE_PAGE}"));
    if placed.is_file() || listed_frame(&placed).is_some() {
        return placed;
    }
    Path::new(products).join(format!("{basename}{REPOSITION_MODULE_PAGE}"))
}

/// One page a frame list offers: where its bytes are and the digest they must have.
#[derive(Clone)]
struct ListedFrame {
    segment: PathBuf,
    offset: u64,
    length: usize,
    content: [u8; 32],
}

/// The frame list this worker parsed last, by its path, size and modification time: a request reads one list for
/// every page it stages.
type ParsedList = (PathBuf, u64, SystemTime, Arc<HashMap<String, ListedFrame>>);
static LISTED: Mutex<Option<ParsedList>> = Mutex::new(None);

/// The frame the list of a staged page's directory offers for it, if it names the page.
fn listed_frame(path: &Path) -> Option<ListedFrame> {
    let list = path.parent()?.join(FRAME_LIST);
    let name = path.file_name()?.to_str()?;
    let metadata = std::fs::metadata(&list).ok()?;
    let (size, modified) = (metadata.len(), metadata.modified().ok()?);
    if let Ok(guard) = LISTED.lock()
        && let Some((kept, kept_size, kept_modified, frames)) = guard.as_ref()
        && *kept == list
        && *kept_size == size
        && *kept_modified == modified
    {
        return frames.get(name).cloned();
    }
    let frames = Arc::new(read_frame_list(&list)?);
    let found = frames.get(name).cloned();
    if let Ok(mut guard) = LISTED.lock() {
        *guard = Some((list, size, modified, frames));
    }
    found
}

/// Every page a frame list names; none when any line is malformed.
fn read_frame_list(list: &Path) -> Option<HashMap<String, ListedFrame>> {
    let text = std::fs::read_to_string(list).ok()?;
    parse_frame_list(&text)
}

/// Interpret the canonical staged frame columns without reading their payloads.
fn parse_frame_list(text: &str) -> Option<HashMap<String, ListedFrame>> {
    let mut frames = HashMap::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [name, segment, offset, length, content] = fields.as_slice() else {
            return None;
        };
        let content: [u8; 32] = hex::decode(content).ok()?.try_into().ok()?;
        let frame = ListedFrame {
            segment: PathBuf::from(segment),
            offset: offset.parse().ok()?,
            length: length.parse().ok()?,
            content,
        };
        frames.insert((*name).to_owned(), frame);
    }
    Some(frames)
}

/// A staged page's bytes: its copy, or the store frame range its directory's frame list names. The caller checks them
/// against the page's content digest.
fn staged_bytes(path: &Path) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            framed_bytes(&listed_frame(path)?)
        }
        Err(_) => None,
    }
}

/// A staged page's bytes for a reader outside the worker's kept pages: its copy, or the store frame range its
/// companion names, which is read only with the content digest the page is named by and only when it matches.
pub(crate) fn staged_page(path: &Path) -> Result<Vec<u8>, String> {
    count_staged_read(path);
    let bytes =
        staged_bytes(path).ok_or_else(|| format!("cannot read staged page {}", path.display()))?;
    match named_content(path) {
        Some(content) if <[u8; 32]>::from(Sha256::digest(&bytes)) == content => Ok(bytes),
        Some(_) => Err(format!(
            "staged page {} differs from its named digest",
            path.display()
        )),
        None if path.is_file() => Ok(bytes),
        None => Err(format!(
            "staged page {} is offered by its frame without a digest",
            path.display()
        )),
    }
}

/// An admitted module's plan page with its events: the page this worker already read and checked for the content
/// Oven names it by, else read now through its copy or frame, checked against that digest and kept. A copy Oven names
/// no digest for is read for this request only.
pub(crate) fn admitted_plan_page(path: &Path) -> Result<Arc<PlanModuleValue>, String> {
    let read = || {
        let bytes = staged_page(path)?;
        let page = decode_plan_module(&bytes)
            .map_err(|error| format!("admitted plan module refused: {error:?}"))?;
        Ok((page, bytes.len() as u64))
    };
    match named_content(path) {
        Some(content) => resident_full_page(&content, read),
        None => read().map(|(page, _)| Arc::new(page)),
    }
}

/// How many times this process read each staged page, for counted contracts.
#[cfg(test)]
static STAGED_READS: Mutex<Option<HashMap<PathBuf, u64>>> = Mutex::new(None);

#[cfg(test)]
fn count_staged_read(path: &Path) {
    if let Ok(mut guard) = STAGED_READS.lock() {
        *guard
            .get_or_insert_with(HashMap::new)
            .entry(path.to_path_buf())
            .or_insert(0) += 1;
    }
}

#[cfg(not(test))]
fn count_staged_read(_path: &Path) {}

/// How many times this process has read and checked the staged page at `path`.
#[cfg(test)]
pub(crate) fn staged_page_reads(path: &Path) -> u64 {
    STAGED_READS
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().and_then(|reads| reads.get(path).copied()))
        .unwrap_or(0)
}

/// The bytes of the store frame range a frame list names.
fn framed_bytes(frame: &ListedFrame) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(&frame.segment).ok()?;
    file.seek(SeekFrom::Start(frame.offset)).ok()?;
    let mut bytes = vec![0; frame.length];
    file.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

/// An admitted module's final events by native function name, and whether its bodies must be placed at the current
/// source's node positions.
pub struct AdmittedPage {
    pub events: HashMap<String, PlanModuleFunction>,
    pub inspect_placement: bool,
}

impl AdmittedPage {
    /// The final event of the body lowered to this native function, when the page holds one. The name carries no
    /// source position, so a body that moved in its file, as under an added comment, still finds its event.
    pub fn event(&self, function: &str) -> Option<&PlanModuleFunction> {
        self.events.get(function)
    }
}

/// One module's page as one request reads it: the page this worker keeps for its content, which holds each event's
/// identity but not its bytes, and where this request's copy of the bytes is, read only for a restore the worker has
/// not kept and only when they match the page's content digest.
pub struct ModulePage {
    resident: Arc<ResidentPage>,
    path: PathBuf,
    content: [u8; 32],
    /// This request's page bytes, read at most once and refused when they do not match `content`.
    encoded: OnceLock<Option<Vec<u8>>>,
    module: String,
    /// This request's page with its event bytes, read on its first restore the worker has not kept.
    full: OnceLock<Option<AdmittedPage>>,
}

impl Deref for ModulePage {
    type Target = ResidentPage;

    fn deref(&self) -> &ResidentPage {
        &self.resident
    }
}

impl ModulePage {
    /// A page that admits nothing.
    fn empty() -> Self {
        Self {
            resident: ResidentPage::unkept(no_page()),
            path: PathBuf::new(),
            content: [0; 32],
            encoded: OnceLock::new(),
            module: String::new(),
            full: OnceLock::new(),
        }
    }

    /// The page this worker keeps for this content.
    pub fn resident(&self) -> &Arc<ResidentPage> {
        &self.resident
    }

    /// The bytes of the final event of the body lowered to `function`, read from this request's page.
    pub fn event_bytes(&self, function: &str) -> Option<&[u8]> {
        let inspect = self.resident.inspect_placement;
        let page = self
            .full
            .get_or_init(|| read_page(self.bytes()?, &self.module, inspect))
            .as_ref()?;
        page.event(function).map(|event| event.event.as_slice())
    }

    /// This request's page bytes, when they match the page's content digest.
    fn bytes(&self) -> Option<&[u8]> {
        page_bytes(&self.encoded, &self.path, &self.content)
    }
}

/// Read a page's bytes into `cell` once, from its copy or its store frame, keeping them only when their digest is
/// `content`.
fn page_bytes<'a>(
    cell: &'a OnceLock<Option<Vec<u8>>>,
    path: &Path,
    content: &[u8; 32],
) -> Option<&'a [u8]> {
    cell.get_or_init(|| {
        let bytes = staged_bytes(path)?;
        (<[u8; 32]>::from(Sha256::digest(&bytes)) == *content).then_some(bytes)
    })
    .as_deref()
}

/// The content digest Oven named a staged page by: its `.digest` companion's 64 lowercase hexadecimal digits, or the
/// digest its directory's frame list names it by.
fn named_content(path: &Path) -> Option<[u8; 32]> {
    let Ok(named) = std::fs::read(format!("{}.digest", path.display())) else {
        return listed_frame(path).map(|frame| frame.content);
    };
    if named.len() != 64
        || !named
            .iter()
            .all(|digit| digit.is_ascii_digit() || (b'a'..=b'f').contains(digit))
    {
        return None;
    }
    hex::decode(&named).ok()?.try_into().ok()
}

/// One request's exact selected graph and module inventory, checked against the digest the selector passed separately.
pub struct PlanModuleSelection {
    digest: [u8; 32],
    record: PlanModuleSelectionRecord,
}

impl PlanModuleSelection {
    /// Read the canonical staged descriptor under the current activation's independent selection authority.
    pub fn read(products: &Path, expected: [u8; 32]) -> Result<Self, String> {
        let path = products.join(PLAN_MODULE_SELECTION_PAGE);
        let mut companion = path.as_os_str().to_os_string();
        companion.push(".digest");
        if read_selection_file(Path::new(&companion), 64)? != hex::encode(expected).as_bytes() {
            return Err("staged plan-module selection does not name the accepted digest".into());
        }
        // Oven forms this descriptor for the activation, so it has no retained store frame to read here.
        let encoded = read_selection_file(&path, MAX_PLAN_MODULE_SELECTION_BYTES)?;
        if <[u8; 32]>::from(Sha256::digest(&encoded)) != expected {
            return Err("staged plan-module selection differs from the accepted digest".into());
        }
        Ok(Self {
            digest: expected,
            record: decode_plan_module_selection(&encoded)?,
        })
    }

    /// The exact digest supplied by the graph selector.
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }

    /// The graph root, view and producer that selected the staged modules.
    pub fn scope(&self) -> &PlanSelectionScope {
        &self.record.scope
    }

    /// Resolve one module in the canonical name order the decoder already verified.
    pub fn module(&self, module: &str) -> Option<&PlanSelectedModule> {
        let modules = &self.record.modules;
        let position = modules
            .binary_search_by(|row| row.module.as_str().cmp(module))
            .ok()?;
        modules.get(position)
    }
}

/// Bound the request-private descriptor and its named digest before allocating their byte buffers.
fn read_selection_file(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(format!(
            "{} exceeds the staged selection file limit of {limit} bytes",
            path.display()
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!(
            "{} exceeds the staged selection file limit of {limit} bytes",
            path.display()
        ));
    }
    Ok(bytes)
}

/// The final events of an admitted module, by the native function name of the body each belongs to.
///
/// Bakery writes the page only for a module whose every witness still validates. No page, or a page that does not
/// decode or names another module, admits nothing: every body then takes the keyed route. A page is known by the
/// digest of its bytes: the one Oven names it by when it stages one, else the digest of the bytes read here. A page
/// this worker already read for that content is the page it kept, with the products it restored from it, and its
/// bytes are not read again.
pub fn admitted_page(products: Option<&str>, module: &str, generation: Option<&str>) -> ModulePage {
    let (Some(directory), Some(_)) = (products, generation) else {
        return ModulePage::empty();
    };
    let path = module_page(directory, module);
    let inspect = path.to_string_lossy().ends_with(REPOSITION_MODULE_PAGE);
    let encoded = OnceLock::new();
    let content = match named_content(&path) {
        Some(content) => content,
        None => {
            let Some(bytes) = staged_bytes(&path) else {
                return ModulePage::empty();
            };
            let content = Sha256::digest(&bytes).into();
            let _ = encoded.set(Some(bytes));
            content
        }
    };
    let full = OnceLock::new();
    let resident = resident_page(&content, module, inspect, || {
        let bytes = page_bytes(&encoded, &path, &content)?;
        let page = read_page(bytes, module, inspect)?;
        let kept = without_event_bytes(&page);
        let _ = full.set(Some(page));
        Some((kept, bytes.len() as u64))
    });
    let Some(resident) = resident else {
        return ModulePage::empty();
    };
    ModulePage {
        resident,
        path,
        content,
        encoded,
        module: module.to_owned(),
        full,
    }
}

/// A page that admits nothing.
fn no_page() -> AdmittedPage {
    AdmittedPage {
        events: HashMap::new(),
        inspect_placement: true,
    }
}

/// Decode a module page, refusing one that names another module.
fn read_page(encoded: &[u8], module: &str, inspect: bool) -> Option<AdmittedPage> {
    let record = decode_plan_module(encoded).ok()?;
    if record.module != module {
        return None;
    }
    let mut page = AdmittedPage {
        events: HashMap::new(),
        inspect_placement: inspect,
    };
    for function in record.functions {
        page.events.insert(function.function.clone(), function);
    }
    Some(page)
}

/// The page a worker keeps: each event's identity, without its bytes.
fn without_event_bytes(page: &AdmittedPage) -> AdmittedPage {
    let events = page
        .events
        .iter()
        .map(|(name, function)| {
            let kept = PlanModuleFunction {
                identity: function.identity.clone(),
                symbol: function.symbol.clone(),
                function: function.function.clone(),
                key: function.key.clone(),
                event: Vec::new(),
            };
            (name.clone(), kept)
        })
        .collect();
    AdmittedPage {
        events,
        inspect_placement: page.inspect_placement,
    }
}

/// Distinguish fully admitted modules from modules with individual restored functions.
fn note_suffix(complete: bool) -> &'static str {
    if complete {
        ADMITTED_MODULE_NOTE
    } else {
        PARTIAL_MODULE_NOTE
    }
}

/// Settled module admissions from a single compilation request.
#[derive(Default)]
pub struct PlanAdmissions {
    modules: BTreeMap<String, SettledModule>,
}

impl PlanAdmissions {
    /// Borrow the exact module admissions established by lowering.
    pub(crate) fn modules(&self) -> &BTreeMap<String, SettledModule> {
        &self.modules
    }
}

/// A module's exact admission note and the native inputs established for its functions, in the same order.
pub(crate) struct SettledModule {
    pub note: AdmittedModule,
    pub products: Vec<AdmittedProduct>,
}

/// The exact product identity and helper inventory established while admitting one native function.
#[cfg_attr(test, derive(Clone))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AdmittedProduct {
    pub key: String,
    pub digest: String,
    pub helpers: Vec<String>,
}

/// Publish the Oven note for a restored module and transfer its checked native inputs to this request's publisher.
#[allow(clippy::too_many_arguments)]
pub(crate) fn report_admitted(
    admissions: &mut PlanAdmissions,
    report: Option<&str>,
    module: &str,
    functions: Vec<AdmittedFunction>,
    evidence: Vec<AdmittedProduct>,
    ordinary: usize,
    inspected: bool,
    products: Option<&str>,
) -> Result<(), String> {
    let (Some(report), Some(directory)) = (report, products) else {
        return Ok(());
    };
    validate_admitted_products(module, &functions, &evidence)?;
    let basename = hex::encode(module.as_bytes());
    let complete = functions.len() == ordinary;
    let selected = note_suffix(complete);
    let private = Path::new(directory).join("admissions");
    let root = Path::new(report);
    for suffix in [ADMITTED_MODULE_NOTE, PARTIAL_MODULE_NOTE] {
        if suffix != selected || functions.is_empty() {
            let path = private.join(format!("{basename}{suffix}"));
            for stale in [
                placement_path(&path),
                path,
                root.join(format!("{basename}{suffix}")),
            ] {
                if stale.is_file() {
                    std::fs::remove_file(&stale).map_err(|error| error.to_string())?;
                }
            }
        }
    }
    if functions.is_empty() {
        admissions.modules.remove(module);
        return Ok(());
    }
    let name = format!("{basename}{selected}");
    let note = AdmittedModule {
        module: module.to_owned(),
        complete,
        functions,
        inspected,
    };
    write_admitted_note(&private.join(&name), &root.join(&name), &note)?;
    admissions.modules.insert(
        module.to_owned(),
        SettledModule {
            note,
            products: evidence,
        },
    );
    Ok(())
}

/// Reject an incomplete or ambiguous module admission before either consumer sees it.
fn validate_admitted_products(
    module: &str,
    functions: &[AdmittedFunction],
    products: &[AdmittedProduct],
) -> Result<(), String> {
    if products.len() != functions.len() {
        return Err("admitted products are not aligned with the restored functions".into());
    }
    let mut names = HashSet::new();
    for (function, product) in functions.iter().zip(products) {
        if function.function.is_empty()
            || !names.insert(&function.function)
            || !product_hex(&product.key)
            || !product
                .digest
                .strip_prefix("sha256:")
                .is_some_and(product_hex)
            || function
                .placement
                .as_ref()
                .is_some_and(|placement| placement.key != product.key)
        {
            return Err(format!(
                "admitted products of {module} disagree with its note"
            ));
        }
    }
    Ok(())
}

/// Write the request-local note Oven reads, with its sparse positions and the shared descriptor's diagnostic copy.
fn write_admitted_note(
    path: &Path,
    diagnostic: &Path,
    note: &AdmittedModule,
) -> Result<(), String> {
    let encoded = encode_module_note(note)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    write_module_placement(&placement_path(path).to_string_lossy(), note)?;
    std::fs::write(path, &encoded).map_err(|error| error.to_string())?;
    std::fs::write(diagnostic, &encoded).map_err(|error| error.to_string())
}

/// The coordinates beside the note Oven reads.
fn placement_path(note: &Path) -> PathBuf {
    PathBuf::from(format!("{}.placement", note.display()))
}

/// Match a complete lowercase SHA-256 representation used by native product keys and digests.
fn product_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Read the module notes the request made available to Oven.
#[cfg(test)]
fn read_admitted(directory: &Path) -> Result<Vec<AdmittedModule>, String> {
    let mut notes = Vec::new();
    if !directory.is_dir() {
        return Ok(notes);
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(directory)
        .map_err(|error| error.to_string())?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<_, _>>()?;
    paths.sort_by_key(|path| path.to_string_lossy().into_owned());
    for path in paths {
        let text = path.to_string_lossy();
        if !text.ends_with(ADMITTED_MODULE_NOTE) && !text.ends_with(PARTIAL_MODULE_NOTE) {
            continue;
        }
        let note = incan_mir_plan_native::read_module_admission(&path.to_string_lossy())?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        let expected = format!(
            "{}{}",
            hex::encode(note.module.as_bytes()),
            note_suffix(note.complete)
        );
        if name.as_deref() != Some(expected.as_str()) {
            return Err("module admission note disagrees with its filename".into());
        }
        notes.push(note);
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use incan_mir_plan_native::{
        AdmittedFunction, AdmittedPlacement, EventPosition, EventReference, Function, Local,
        LoweringEvent, MAX_PLAN_MODULE_SELECTION_BYTES, PLAN_MODULE_SELECTION_PAGE,
        PlanModuleFunction, PlanModuleValue, PlanType, TerminatorKind, block, encode_plan_module,
        encode_product, source_span,
    };

    use super::{
        AdmittedProduct, FRAME_LIST, PlanAdmissions, PlanModuleSelection, admitted_page,
        admitted_plan_page, read_admitted, report_admitted, staged_page, staged_page_reads,
    };
    use crate::record_positions::{indexed_product, placed_record};
    use crate::resident_products::RestoreInputs;
    use crate::scalar_store::AdmittedPayload;
    use sha2::{Digest, Sha256};

    /// Offer the page at `path` by a frame range of `segment` in its directory's frame list, named by `digest`,
    /// replacing any line the list had for it.
    fn list_frame(
        path: &Path,
        segment: &Path,
        offset: usize,
        length: usize,
        digest: &[u8],
    ) -> Result<(), String> {
        let list = path
            .parent()
            .ok_or("page has no directory")?
            .join(FRAME_LIST);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("page has no name")?;
        let kept = std::fs::read_to_string(&list).unwrap_or_default();
        let mut lines: Vec<String> = kept
            .lines()
            .filter(|line| line.split('\t').next() != Some(name))
            .map(str::to_owned)
            .collect();
        lines.push(format!(
            "{name}\t{}\t{offset}\t{length}\t{}",
            segment.display(),
            hex::encode(digest)
        ));
        std::fs::write(&list, lines.join("\n") + "\n").map_err(|error| error.to_string())
    }

    /// A fresh directory for one test.
    pub(super) fn directory(test: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("incan-plan-modules-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("directory");
        path
    }

    fn restored(function: &str, placed: bool) -> AdmittedFunction {
        AdmittedFunction {
            function: function.into(),
            identity: format!("pub::lone_alpha::{function}"),
            references: vec![EventReference {
                kind: "model".into(),
                index: 1,
                name: "Point".into(),
            }],
            placement: placed.then(|| AdmittedPlacement {
                key: "ab".repeat(32),
                positions: vec![EventPosition { line: 1, column: 5 }],
            }),
        }
    }

    /// Give a test restore the product evidence it would establish when admitting its exact page.
    fn evidence(helpers: &[&str]) -> AdmittedProduct {
        AdmittedProduct {
            key: "ab".repeat(32),
            digest: format!("sha256:{}", "cd".repeat(32)),
            helpers: helpers.iter().map(|helper| (*helper).to_owned()).collect(),
        }
    }

    fn names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn a_whole_module_note_replaces_a_partial_one_and_reads_back() -> Result<(), String> {
        let root = directory("notes");
        let (report, products) = (root.join("report"), root.join("products"));
        std::fs::create_dir_all(&report).map_err(|error| error.to_string())?;
        let module = "lone_alpha";
        let basename = hex::encode(module);
        let mut request = PlanAdmissions::default();
        let report_text = report.to_string_lossy();
        let products_text = products.to_string_lossy();
        let partial = vec![restored("a", false)];
        report_admitted(
            &mut request,
            Some(&report_text),
            module,
            partial,
            vec![evidence(&[])],
            2,
            false,
            Some(&products_text),
        )?;
        assert_eq!(names(&report), [format!("{basename}.partially-admitted")]);
        let whole = vec![restored("a", false), restored("b", true)];
        let carried = vec![evidence(&[]), evidence(&["helper"])];
        report_admitted(
            &mut request,
            Some(&report_text),
            module,
            whole.clone(),
            carried.clone(),
            2,
            true,
            Some(&products_text),
        )?;
        assert_eq!(names(&report), [format!("{basename}.admitted")]);
        let admissions = products.join("admissions");
        assert_eq!(
            names(&admissions),
            [
                format!("{basename}.admitted"),
                format!("{basename}.admitted.placement")
            ]
        );
        let notes = read_admitted(&admissions)?;
        assert_eq!(notes.len(), 1);
        assert!(notes[0].complete && notes[0].inspected);
        assert_eq!(notes[0].functions, whole);
        {
            let stored = request
                .modules()
                .get(module)
                .ok_or("request did not retain its admission")?;
            assert_eq!(stored.note.functions, whole);
            assert_eq!(stored.products, carried);
        }
        assert!(
            report_admitted(
                &mut request,
                Some(&report_text),
                module,
                whole,
                Vec::new(),
                2,
                true,
                Some(&products_text)
            )
            .is_err()
        );
        report_admitted(
            &mut request,
            Some(&report_text),
            module,
            Vec::new(),
            Vec::new(),
            2,
            true,
            Some(&products_text),
        )?;
        assert!(request.modules().is_empty());
        assert!(names(&report).is_empty());
        assert!(read_admitted(&admissions)?.is_empty());
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn a_note_under_another_module_name_refuses() -> Result<(), String> {
        let root = directory("renamed");
        let (report, products) = (root.join("report"), root.join("products"));
        std::fs::create_dir_all(&report).map_err(|error| error.to_string())?;
        let mut request = PlanAdmissions::default();
        let report_text = report.to_string_lossy();
        let products_text = products.to_string_lossy();
        report_admitted(
            &mut request,
            Some(&report_text),
            "lone_alpha",
            vec![restored("a", false)],
            vec![evidence(&[])],
            1,
            false,
            Some(&products_text),
        )?;
        let admissions = products.join("admissions");
        let from = admissions.join(format!("{}.admitted", hex::encode("lone_alpha")));
        let to = admissions.join(format!("{}.admitted", hex::encode("lone_beta")));
        std::fs::rename(&from, &to).map_err(|error| error.to_string())?;
        // A note without changed placements and without inspection has no placement companion.
        assert!(!PathBuf::from(format!("{}.placement", from.display())).exists());
        assert!(read_admitted(&admissions).is_err());
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn only_a_profiled_page_of_the_same_module_admits() {
        let root = directory("pages");
        let products = root.to_string_lossy().into_owned();
        let page = PlanModuleValue {
            module: "lone_alpha".into(),
            functions: vec![PlanModuleFunction {
                identity: "pub::lone_alpha::a".into(),
                symbol: "pub::lone_alpha::a".into(),
                function: "a".into(),
                key: "ef".repeat(32),
                event: vec![1],
            }],
        };
        let encoded = encode_plan_module(&page).expect("page");
        let basename = hex::encode("lone_alpha");
        std::fs::write(
            root.join(format!("{basename}.incnmodule.reposition")),
            &encoded,
        )
        .expect("write");
        assert!(
            admitted_page(Some(&products), "lone_alpha", None)
                .events
                .is_empty()
        );
        let admitted = admitted_page(Some(&products), "lone_alpha", Some("profile"));
        assert!(admitted.inspect_placement);
        assert_eq!(
            admitted.event("a").map(|event| event.key.clone()),
            Some("ef".repeat(32))
        );
        std::fs::write(
            root.join(format!("{basename}.incnmodule.positioned")),
            &encoded,
        )
        .expect("write");
        assert!(!admitted_page(Some(&products), "lone_alpha", Some("profile")).inspect_placement);
        let other = hex::encode("lone_beta");
        std::fs::write(
            root.join(format!("{other}.incnmodule.positioned")),
            &encoded,
        )
        .expect("write");
        assert!(
            admitted_page(Some(&products), "lone_beta", Some("profile"))
                .events
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A placed product for a restored page entry, its requests and the event it came from.
    fn restored_payload() -> Result<AdmittedPayload, String> {
        let owner = source_span("resident.incn", 10, 1);
        let exit = block(
            Vec::new(),
            TerminatorKind::Return,
            &source_span("resident.incn", 11, 5),
        );
        let locals = vec![Local {
            ty: PlanType::Unit,
            span: owner.clone(),
        }];
        let function = Function::new(
            "a".into(),
            Vec::new(),
            PlanType::Unit,
            locals,
            vec![exit],
            owner,
        );
        let wire = encode_product(&function, &[], &[], &[])?;
        let stored = indexed_product(&wire, &[(0, 1), (1, 5)], "resident.incn", 10)?;
        let placed = placed_record(&stored, &[(0, 1), (1, 5)], "resident.incn", 30)?;
        let event = LoweringEvent {
            identity: "pub::resident_gamma::a".into(),
            key: "cd".repeat(32),
            function: "a".into(),
            product: Vec::new(),
            reused: false,
            admitted: true,
            module: "resident_gamma".into(),
            role: "ordinary".into(),
            demand: String::new(),
            runtime_requirements: 0,
            references: Vec::new(),
            positions: Vec::new(),
        };
        Ok(AdmittedPayload {
            function: placed,
            requests: Vec::new(),
            event,
            coordinates_match: true,
            digest: evidence(&[]).digest,
        })
    }

    /// A resident worker reads an unchanged page once and restores each of its products once per restore inputs.
    #[test]
    fn a_resident_worker_reads_an_unchanged_page_once() -> Result<(), String> {
        let root = directory("resident");
        let products = root.to_string_lossy().into_owned();
        let page = PlanModuleValue {
            module: "resident_gamma".into(),
            functions: vec![PlanModuleFunction {
                identity: "pub::resident_gamma::a".into(),
                symbol: "pub::resident_gamma::a".into(),
                function: "a".into(),
                key: "cd".repeat(32),
                event: vec![2],
            }],
        };
        let encoded = encode_plan_module(&page)?;
        let basename = hex::encode("resident_gamma");
        let reposition = root.join(format!("{basename}.incnmodule.reposition"));
        std::fs::write(&reposition, &encoded).map_err(|error| error.to_string())?;
        let first = admitted_page(Some(&products), "resident_gamma", Some("profile"));
        let again = admitted_page(Some(&products), "resident_gamma", Some("profile"));
        assert!(
            Arc::ptr_eq(first.resident(), again.resident()),
            "an unchanged page was read again"
        );
        assert!(first.inspect_placement && first.event("a").is_some());
        // The kept page holds no event bytes; each request reads them from its own copy of the page.
        assert!(first.event("a").is_some_and(|event| event.event.is_empty()));
        assert_eq!(again.event_bytes("a"), Some([2_u8].as_slice()));
        // The same bytes under another module name stay refused, and placed bytes are another page.
        let other = root.join(format!(
            "{}.incnmodule.positioned",
            hex::encode("resident_delta")
        ));
        std::fs::write(&other, &encoded).map_err(|error| error.to_string())?;
        assert!(
            admitted_page(Some(&products), "resident_delta", Some("profile"))
                .events
                .is_empty()
        );
        let positioned = root.join(format!("{basename}.incnmodule.positioned"));
        std::fs::write(&positioned, &encoded).map_err(|error| error.to_string())?;
        let placed = admitted_page(Some(&products), "resident_gamma", Some("profile"));
        assert!(!Arc::ptr_eq(first.resident(), placed.resident()) && !placed.inspect_placement);
        let inputs = RestoreInputs {
            frame: "a".into(),
            file: "resident.incn".into(),
            line: 30,
            overflow: true,
            reads: "reads".into(),
            generation: "profile".into(),
            positions: Some(vec![(0, 1), (1, 5)]),
        };
        let mut restores = 0;
        let made = first.restored(&inputs, || {
            restores += 1;
            restored_payload()
        })?;
        let kept = first.restored(&inputs, || {
            restores += 1;
            restored_payload()
        })?;
        assert!(
            Arc::ptr_eq(&made, &kept) && restores == 1,
            "a kept product was restored again"
        );
        let moved = RestoreInputs {
            line: 31,
            ..inputs.clone()
        };
        first.restored(&moved, || {
            restores += 1;
            restored_payload()
        })?;
        assert_eq!(
            restores, 2,
            "a product restored at another line was read from the worker"
        );
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// A page Oven names by the digest of its bytes is found by that name: the worker does not read it again while the
    /// name is unchanged, and refuses bytes that do not match the name when it does read them.
    #[test]
    fn a_page_named_by_its_identity_is_not_read_again() -> Result<(), String> {
        let root = directory("named");
        let products = root.to_string_lossy().into_owned();
        let page = PlanModuleValue {
            module: "named_epsilon".into(),
            functions: vec![PlanModuleFunction {
                identity: "pub::named_epsilon::a".into(),
                symbol: "pub::named_epsilon::a".into(),
                function: "a".into(),
                key: "ab".repeat(32),
                event: vec![3],
            }],
        };
        let encoded = encode_plan_module(&page)?;
        let path = root.join(format!(
            "{}.incnmodule.reposition",
            hex::encode("named_epsilon")
        ));
        let named = PathBuf::from(format!("{}.digest", path.display()));
        let write = |path: &Path, bytes: &[u8]| {
            std::fs::write(path, bytes).map_err(|error| error.to_string())
        };
        write(&path, &encoded)?;
        write(&named, hex::encode(Sha256::digest(&encoded)).as_bytes())?;
        let first = admitted_page(Some(&products), "named_epsilon", Some("profile"));
        assert!(first.event("a").is_some());
        // Bytes that no longer match the name are not read while the name is unchanged, and refused when they are.
        write(&path, b"not a page")?;
        let again = admitted_page(Some(&products), "named_epsilon", Some("profile"));
        assert!(
            Arc::ptr_eq(first.resident(), again.resident()),
            "a page named by its identity was read again"
        );
        assert_eq!(again.event_bytes("a"), None);
        // A name that does not match the bytes on a first read admits nothing.
        let other = root.join(format!(
            "{}.incnmodule.reposition",
            hex::encode("named_zeta")
        ));
        write(
            &other,
            &encode_plan_module(&PlanModuleValue {
                module: "named_zeta".into(),
                ..page.clone()
            })?,
        )?;
        write(
            &PathBuf::from(format!("{}.digest", other.display())),
            "cd".repeat(32).as_bytes(),
        )?;
        assert!(
            admitted_page(Some(&products), "named_zeta", Some("profile"))
                .events
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// A worker reads, checks and decodes a staged page once per content Oven names it by, and keeps it with its
    /// events: a request over the same pages reads none of them again, and one changed page is the one page read and
    /// hashed.
    #[test]
    fn a_worker_reads_and_hashes_one_page_for_one_changed_page() -> Result<(), String> {
        let root = directory("resident-full-pages");
        let write = |path: &Path, bytes: &[u8]| {
            std::fs::write(path, bytes).map_err(|error| error.to_string())
        };
        let page = |module: &str, event: u8| PlanModuleValue {
            module: module.into(),
            functions: vec![PlanModuleFunction {
                identity: format!("pub::{module}::a"),
                symbol: format!("pub::{module}::a"),
                function: "a".into(),
                key: "cd".repeat(32),
                event: vec![event],
            }],
        };
        let offer = |name: &str, encoded: &[u8]| -> Result<PathBuf, String> {
            let segment = root.join(format!(
                "{name}-{}.seg",
                hex::encode(&Sha256::digest(encoded)[..4])
            ));
            let mut stored = b"key prefix".to_vec();
            let offset = stored.len();
            stored.extend_from_slice(encoded);
            write(&segment, &stored)?;
            let path = root.join(format!("{}.incnmodule.positioned", hex::encode(name)));
            list_frame(
                &path,
                &segment,
                offset,
                encoded.len(),
                &Sha256::digest(encoded),
            )?;
            Ok(path)
        };
        let first = offer(
            "resident_kappa",
            &encode_plan_module(&page("resident_kappa", 1))?,
        )?;
        let second = offer(
            "resident_lambda",
            &encode_plan_module(&page("resident_lambda", 2))?,
        )?;
        let reads = || staged_page_reads(&first) + staged_page_reads(&second);
        for path in [&first, &second] {
            admitted_plan_page(path)?;
        }
        assert_eq!(reads(), 2);
        for path in [&first, &second] {
            assert_eq!(admitted_plan_page(path)?.functions.len(), 1);
        }
        assert_eq!(reads(), 2, "a page the worker holds was read again");
        // One page changes: the next request reads and hashes that page alone.
        let changed = offer(
            "resident_lambda",
            &encode_plan_module(&page("resident_lambda", 3))?,
        )?;
        assert_eq!(changed, second);
        assert_eq!(admitted_plan_page(&first)?.functions[0].event, vec![1]);
        assert_eq!(admitted_plan_page(&second)?.functions[0].event, vec![3]);
        assert_eq!(reads(), 3, "one changed page was not the one page read");
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// An oversized selection copy or named digest refuses at its own file boundary before reading its bytes.
    #[test]
    fn staged_module_selection_bounds_both_its_files() -> Result<(), String> {
        let root = directory("selection-file-bounds");
        let path = root.join(PLAN_MODULE_SELECTION_PAGE);
        let companion = root.join(format!("{PLAN_MODULE_SELECTION_PAGE}.digest"));
        let expected = [0x55; 32];
        let write = |path: &Path, value: &[u8]| {
            std::fs::write(path, value).map_err(|error| error.to_string())
        };
        write(&companion, hex::encode(expected).as_bytes())?;
        let descriptor = std::fs::File::create(&path).map_err(|error| error.to_string())?;
        descriptor
            .set_len(MAX_PLAN_MODULE_SELECTION_BYTES + 1)
            .map_err(|error| error.to_string())?;
        assert!(matches!(
            PlanModuleSelection::read(&root, expected),
            Err(reason) if reason.contains(&format!("limit of {MAX_PLAN_MODULE_SELECTION_BYTES} bytes"))
        ));
        write(&path, b"incomplete descriptor")?;
        write(&companion, &[b'5'; 65])?;
        assert!(matches!(
            PlanModuleSelection::read(&root, expected),
            Err(reason) if reason.contains("limit of 64 bytes")
        ));
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// Every reader of a staged page, such as the one that appends the events of an admitted module's restored
    /// functions, reads a page Oven offers by its store frame from the segment range, checked against the digest the
    /// page is named by; a copied page is read as before, and a frame without its digest, or with another, is refused.
    #[test]
    fn every_reader_of_a_staged_page_reads_a_frame_offered_page_checked_by_its_digest()
    -> Result<(), String> {
        let root = directory("frame-reader");
        let write = |path: &Path, bytes: &[u8]| {
            std::fs::write(path, bytes).map_err(|error| error.to_string())
        };
        let segment = root.join("one.seg");
        write(&segment, b"prefix page bytes suffix")?;
        let framed = root.join("framed.incnmodule.positioned");
        list_frame(&framed, &segment, 7, 10, &Sha256::digest(b"page bytes"))?;
        assert_eq!(staged_page(&framed)?, b"page bytes".to_vec());
        let copied = root.join("copied.incnmodule.positioned");
        write(&copied, b"copied page")?;
        assert_eq!(staged_page(&copied)?, b"copied page".to_vec());
        let unnamed = root.join("unnamed.incnmodule.positioned");
        assert!(
            staged_page(&unnamed).is_err(),
            "a page with neither a copy nor a frame was read"
        );
        list_frame(&unnamed, &segment, 7, 10, &Sha256::digest(b"other bytes"))?;
        assert!(
            staged_page(&unnamed).is_err(),
            "a frame whose bytes are not its digest was read"
        );
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// A page Oven offers by its store frame is read from the immutable segment range the frame names, never from a
    /// staged copy, and only when its bytes match the content digest Oven names it by.
    #[test]
    fn a_page_offered_by_its_store_frame_is_read_from_the_segment() -> Result<(), String> {
        let root = directory("framed");
        let products = root.to_string_lossy().into_owned();
        let write = |path: &Path, bytes: &[u8]| {
            std::fs::write(path, bytes).map_err(|error| error.to_string())
        };
        let offer = |module: &str, encoded: &[u8], named: &[u8]| -> Result<PathBuf, String> {
            // The page sits inside a store segment between other frames, as a value does.
            let segment = root.join(format!("{module}.seg"));
            let mut stored = b"key prefix of the frame".to_vec();
            let offset = stored.len();
            stored.extend_from_slice(encoded);
            stored.extend_from_slice(b"the next frame");
            write(&segment, &stored)?;
            let path = root.join(format!("{}.incnmodule.reposition", hex::encode(module)));
            list_frame(
                &path,
                &segment,
                offset,
                encoded.len(),
                &Sha256::digest(named),
            )?;
            Ok(path)
        };
        let page = |module: &str| PlanModuleValue {
            module: module.into(),
            functions: vec![PlanModuleFunction {
                identity: format!("pub::{module}::a"),
                symbol: format!("pub::{module}::a"),
                function: "a".into(),
                key: "cd".repeat(32),
                event: vec![5],
            }],
        };
        let encoded = encode_plan_module(&page("framed_eta"))?;
        let path = offer("framed_eta", &encoded, &encoded)?;
        assert!(!path.exists(), "the page was staged as a copy");
        let admitted = admitted_page(Some(&products), "framed_eta", Some("profile"));
        assert!(admitted.event("a").is_some());
        assert_eq!(admitted.event_bytes("a"), Some(&[5u8][..]));
        // A frame whose bytes are not the content the page is named by admits nothing.
        let other = encode_plan_module(&page("framed_theta"))?;
        offer("framed_theta", &other, b"another page")?;
        assert!(
            admitted_page(Some(&products), "framed_theta", Some("profile"))
                .events
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
