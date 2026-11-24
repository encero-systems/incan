//! A module's admitted plan record: the page a worker restores its functions from, and the note of what it restored.

mod compact;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use incan_mir_plan_native::{
    ADMITTED_MODULE_NOTE, AdmittedFunction, AdmittedModule, MAX_PLAN_MODULE_SELECTION_BYTES, PARTIAL_MODULE_NOTE,
    PLAN_MODULE_SELECTION_PAGE, POSITIONED_MODULE_PAGE, PlanModuleFunction, PlanModuleSelectionRecord, PlanModuleValue,
    PlanSelectedModule, PlanSelectionScope, REPOSITION_MODULE_PAGE, WholeModuleAdmission, decode_plan_module,
    decode_plan_module_selection, encode_module_note, encode_whole_module_admission, write_module_placement,
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
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => framed_bytes(&listed_frame(path)?),
        Err(_) => None,
    }
}

/// A staged page's bytes for a reader outside the worker's kept pages: its copy, or the store frame range its
/// companion names, which is read only with the content digest the page is named by and only when it matches.
pub(crate) fn staged_page(path: &Path) -> Result<Vec<u8>, String> {
    count_staged_read(path);
    let bytes = staged_bytes(path).ok_or_else(|| format!("cannot read staged page {}", path.display()))?;
    match named_content(path) {
        Some(content) if <[u8; 32]>::from(Sha256::digest(&bytes)) == content => Ok(bytes),
        Some(_) => Err(format!("staged page {} differs from its named digest", path.display())),
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
        let page = decode_plan_module(&bytes).map_err(|error| format!("admitted plan module refused: {error:?}"))?;
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

    /// The exact content of the positioned page this request admitted, if there was one.
    pub(crate) fn positioned_content(&self) -> Option<[u8; 32]> {
        (!self.path.as_os_str().is_empty() && !self.inspect_placement).then_some(self.content)
    }

    /// Authenticate this request's actual backing before lending a prior positioned function.
    pub(crate) fn current_positioned_bytes(&self) -> bool {
        self.positioned_content().is_some() && self.bytes().is_some()
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
fn page_bytes<'a>(cell: &'a OnceLock<Option<Vec<u8>>>, path: &Path, content: &[u8; 32]) -> Option<&'a [u8]> {
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
    /// Supply an already typed selection for publication tests that use a synthetic independent pin.
    #[cfg(test)]
    pub(crate) fn test_selection(record: PlanModuleSelectionRecord, digest: [u8; 32]) -> Self {
        Self { record, digest }
    }

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
        let position = modules.binary_search_by(|row| row.module.as_str().cmp(module)).ok()?;
        modules.get(position)
    }
}

/// Bound the request-private descriptor and its named digest before allocating their byte buffers.
fn read_selection_file(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
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
    selection: Option<PlanModuleSelection>,
    modules: BTreeMap<String, SettledModule>,
}

impl PlanAdmissions {
    /// Start the lowering request with its independently accepted module selection, when present.
    pub fn with_selection(selection: Option<PlanModuleSelection>) -> Self {
        Self {
            selection,
            modules: BTreeMap::new(),
        }
    }

    /// The independently accepted selection for this entire request.
    pub(crate) fn selection(&self) -> Option<&PlanModuleSelection> {
        self.selection.as_ref()
    }

    /// Borrow the exact module admissions established by lowering.
    pub(crate) fn modules(&self) -> &BTreeMap<String, SettledModule> {
        &self.modules
    }
}

/// A module's exact admission note and the native inputs established for its functions, in the same order.
pub(crate) struct SettledModule {
    pub note: AdmittedModule,
    pub products: Vec<AdmittedProduct>,
    compact: Option<WholeModuleAdmission>,
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
    positioned: Option<[u8; 32]>,
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
            for stale in [placement_path(&path), path, root.join(format!("{basename}{suffix}"))] {
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
    let compact = whole_module_candidate(admissions.selection(), &note, positioned);
    if compact.is_none() {
        write_admitted_note(&private.join(&name), &root.join(&name), &note)?;
    }
    admissions.modules.insert(
        module.to_owned(),
        SettledModule {
            note,
            products: evidence,
            compact,
        },
    );
    Ok(())
}

/// Nominate a complete admission only when the worker used the exact current positioned selection.
fn whole_module_candidate(
    selection: Option<&PlanModuleSelection>,
    note: &AdmittedModule,
    positioned: Option<[u8; 32]>,
) -> Option<WholeModuleAdmission> {
    let selection = selection?;
    let selected = selection.module(&note.module)?;
    let digest = hex::encode(positioned?);
    let count = i64::try_from(note.functions.len()).ok()?;
    if !note.complete
        || note.inspected
        || !selected.positioned
        || selected.count != count
        || selected.digest != digest
        || note.functions.iter().any(|function| function.placement.is_some())
    {
        return None;
    }
    Some(WholeModuleAdmission {
        module: note.module.clone(),
        output: selected.output.clone(),
        digest,
        count,
        selection: hex::encode(selection.digest()),
    })
}

/// Emit a nominated admission once the publisher knows which declarations the scalar events already name.
pub(crate) fn publish_settled_module_note(
    directory: &str,
    report: &Path,
    settled: &SettledModule,
    reported: &HashSet<String>,
) -> Result<Option<[u8; 32]>, String> {
    let Some(compact) = &settled.compact else {
        return Ok(None);
    };
    let basename = format!("{}{ADMITTED_MODULE_NOTE}", hex::encode(&compact.module));
    let private = Path::new(directory).join("admissions").join(&basename);
    let diagnostic = report.join(&basename);
    if settled
        .note
        .functions
        .iter()
        .any(|function| reported.contains(&function.function))
    {
        write_admitted_note(&private, &diagnostic, &settled.note)?;
        return Ok(None);
    }
    let encoded = encode_whole_module_admission(compact)?;
    prepare_note_directory(&private)?;
    match std::fs::remove_file(placement_path(&private)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    write_note_bytes(&private, &diagnostic, &encoded)?;
    Ok(Some(Sha256::digest(&encoded).into()))
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
            || !product.digest.strip_prefix("sha256:").is_some_and(product_hex)
            || function
                .placement
                .as_ref()
                .is_some_and(|placement| placement.key != product.key)
        {
            return Err(format!("admitted products of {module} disagree with its note"));
        }
    }
    Ok(())
}

/// Write the request-local note Oven reads, with its sparse positions and the shared descriptor's diagnostic copy.
fn write_admitted_note(path: &Path, diagnostic: &Path, note: &AdmittedModule) -> Result<(), String> {
    let encoded = encode_module_note(note)?;
    prepare_note_directory(path)?;
    write_module_placement(&placement_path(path).to_string_lossy(), note)?;
    write_note_bytes(path, diagnostic, &encoded)
}

/// Ensure the request-private admission directory exists before emitting either note shape.
fn prepare_note_directory(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Preserve the exact request-private bytes in the public diagnostic copy.
fn write_note_bytes(path: &Path, diagnostic: &Path, encoded: &[u8]) -> Result<(), String> {
    std::fs::write(path, encoded).map_err(|error| error.to_string())?;
    std::fs::write(diagnostic, encoded).map_err(|error| error.to_string())
}

/// The coordinates beside the note Oven reads.
fn placement_path(note: &Path) -> PathBuf {
    PathBuf::from(format!("{}.placement", note.display()))
}

/// Match a complete lowercase SHA-256 representation used by native product keys and digests.
fn product_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
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
        .map(|entry| entry.map(|entry| entry.path()).map_err(|error| error.to_string()))
        .collect::<Result<_, _>>()?;
    paths.sort_by_key(|path| path.to_string_lossy().into_owned());
    for path in paths {
        let text = path.to_string_lossy();
        if !text.ends_with(ADMITTED_MODULE_NOTE) && !text.ends_with(PARTIAL_MODULE_NOTE) {
            continue;
        }
        let note = incan_mir_plan_native::read_module_admission(&path.to_string_lossy())?;
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned());
        let expected = format!("{}{}", hex::encode(note.module.as_bytes()), note_suffix(note.complete));
        if name.as_deref() != Some(expected.as_str()) {
            return Err("module admission note disagrees with its filename".into());
        }
        notes.push(note);
    }
    Ok(notes)
}

#[cfg(test)]
mod tests;
