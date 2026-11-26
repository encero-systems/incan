//! Anchored native-closure references.
//!
//! A published native closure names each native input from one of three anchors: the Loaf's own directory
//! (`sibling`), the selected toolchain directory (`toolchain`) or the bakery root (`bakery`). Only a path under none of
//! them is written `absolute`. Homes that share one bakery and install the same toolchain therefore publish the same
//! closure bytes, and each reader resolves the anchors against its own toolchain and bakery.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{LoafIndexError, LoafNativeClosure, located};

/// The schema of a published native closure whose references are anchored. A closure without a schema is a legacy
/// publication: a relative path names its target from the Loaf's directory and an absolute path is used as written.
pub const NATIVE_CLOSURE_SCHEMA: &str = "incan.native-closure/2";

/// The directories of the current home that anchored references resolve against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeAnchors {
    /// The selected toolchain directory.
    pub toolchain: Option<PathBuf>,
    /// The root of the bakery the compilation reads and publishes into.
    pub bakery: Option<PathBuf>,
}

/// Where one published reference is anchored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Anchor {
    /// Relative to the publishing Loaf's directory.
    Sibling,
    /// Relative to the selected toolchain directory.
    Toolchain,
    /// Relative to the bakery root.
    Bakery,
    /// A path under none of the other anchors, as written.
    Absolute,
}

impl Anchor {
    /// Every anchor, in the order a reference's prefix is matched.
    const ALL: [Self; 4] = [Self::Sibling, Self::Toolchain, Self::Bakery, Self::Absolute];

    /// The reference prefix naming this anchor.
    fn name(self) -> &'static str {
        match self {
            Self::Sibling => "sibling",
            Self::Toolchain => "toolchain",
            Self::Bakery => "bakery",
            Self::Absolute => "absolute",
        }
    }

    /// The anchor a reference names and the path after its prefix.
    fn parse(reference: &str) -> Option<(Self, &str)> {
        let (name, path) = reference.split_once(':')?;
        Self::ALL
            .into_iter()
            .find(|anchor| anchor.name() == name)
            .map(|anchor| (anchor, path))
    }
}

/// A native closure as `<name>.native.json` spells it: anchored references in place of paths.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedNativeClosure {
    /// [`NATIVE_CLOSURE_SCHEMA`], or absent for a legacy publication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// The crates the unit links, by crate name and reference.
    pub externs: Vec<(String, String)>,
    /// The directories the linked crates' own dependencies are found in.
    pub search: Vec<String>,
    /// The crates a consumer must name directly because the unit exports attribute macros.
    #[serde(default)]
    pub macro_externs: Vec<(String, String)>,
}

impl LoafNativeClosure {
    /// The closure as the Loaf in `directory` publishes it. A path under the selected toolchain or the bakery is
    /// anchored there; a path under the directory's grandparent, the artifact root its sibling Loaves share, is
    /// anchored at the Loaf; any other absolute path is written as it is.
    pub fn published(&self, directory: &Path, anchors: &NativeAnchors) -> Result<PublishedNativeClosure, String> {
        let roots = Roots::new(directory, anchors);
        let pairs = |items: &[(String, PathBuf)]| {
            items
                .iter()
                .map(|(name, path)| Ok((name.clone(), roots.reference(path)?)))
                .collect::<Result<Vec<_>, String>>()
        };
        Ok(PublishedNativeClosure {
            schema: Some(NATIVE_CLOSURE_SCHEMA.to_owned()),
            externs: pairs(&self.externs)?,
            search: self
                .search
                .iter()
                .map(|path| roots.reference(path))
                .collect::<Result<_, _>>()?,
            macro_externs: pairs(&self.macro_externs)?,
        })
    }

    /// Resolve a published closure read from the Loaf in `directory` against the current home's `anchors`.
    pub(super) fn resolved(
        published: PublishedNativeClosure,
        directory: &Path,
        anchors: &NativeAnchors,
    ) -> Result<Self, String> {
        let anchored = match published.schema.as_deref() {
            None => false,
            Some(NATIVE_CLOSURE_SCHEMA) => true,
            Some(other) => return Err(format!("unsupported native closure schema `{other}`")),
        };
        let resolve = |reference: &str| resolved_reference(reference, anchored, directory, anchors);
        let pairs = |items: Vec<(String, String)>| {
            items
                .into_iter()
                .map(|(name, reference)| Ok((name, resolve(&reference)?)))
                .collect::<Result<Vec<_>, String>>()
        };
        Ok(Self {
            externs: pairs(published.externs)?,
            search: published
                .search
                .iter()
                .map(|reference| resolve(reference))
                .collect::<Result<_, _>>()?,
            macro_externs: pairs(published.macro_externs)?,
        })
    }

    /// Read and resolve the native closure of the Loaf `name` in `dir`.
    pub(super) fn read_anchored(dir: &Path, name: &str, anchors: &NativeAnchors) -> Result<Self, LoafIndexError> {
        let path = dir.join(format!("{}.native.json", incan_loaf_names::artifact_stem(name)));
        let unreadable = |reason: String| LoafIndexError::Unreadable {
            path: path.clone(),
            reason,
        };
        let bytes = std::fs::read(&path).map_err(|error| unreadable(error.to_string()))?;
        let published: PublishedNativeClosure =
            serde_json::from_slice(&bytes).map_err(|error| unreadable(error.to_string()))?;
        Self::resolved(published, dir, anchors).map_err(unreadable)
    }
}

/// The anchor roots one publication matches its paths against.
struct Roots<'a> {
    sibling: Option<&'a Path>,
    toolchain: Option<(&'a Path, Option<PathBuf>)>,
    bakery: Option<(&'a Path, Option<PathBuf>)>,
}

impl<'a> Roots<'a> {
    /// Capture each root as given and canonically, so a path spelled through either form is anchored.
    fn new(directory: &'a Path, anchors: &'a NativeAnchors) -> Self {
        let both = |root: &'a Option<PathBuf>| root.as_deref().map(|root| (root, std::fs::canonicalize(root).ok()));
        Self {
            sibling: directory.parent().and_then(Path::parent),
            toolchain: both(&anchors.toolchain),
            bakery: both(&anchors.bakery),
        }
    }

    /// The reference naming `path` from the most specific anchor it is under.
    fn reference(&self, path: &Path) -> Result<String, String> {
        for (anchor, root) in [(Anchor::Toolchain, &self.toolchain), (Anchor::Bakery, &self.bakery)] {
            if let Some(relative) = root
                .as_ref()
                .and_then(|(root, canonical)| beneath(path, root, canonical))
            {
                return spelled(anchor, &relative);
            }
        }
        if let Some(relative) = self.sibling.and_then(|root| path.strip_prefix(root).ok()) {
            return spelled(Anchor::Sibling, &Path::new("../..").join(relative));
        }
        if path.is_absolute() {
            return spelled(Anchor::Absolute, path);
        }
        // A relative input already names its target from the Loaf's directory.
        spelled(Anchor::Sibling, path)
    }
}

/// `path` relative to `root`, comparing canonical spellings when the given ones differ.
fn beneath(path: &Path, root: &Path, canonical: &Option<PathBuf>) -> Option<PathBuf> {
    if let Ok(relative) = path.strip_prefix(root) {
        return Some(relative.to_path_buf());
    }
    let canonical = canonical.as_deref()?;
    let resolved = std::fs::canonicalize(path).ok()?;
    resolved.strip_prefix(canonical).ok().map(Path::to_path_buf)
}

/// The text of one anchored reference; a non-UTF-8 path cannot be published.
fn spelled(anchor: Anchor, path: &Path) -> Result<String, String> {
    let text = path
        .to_str()
        .ok_or_else(|| format!("native closure path {} is not UTF-8", path.display()))?;
    Ok(format!("{}:{text}", anchor.name()))
}

/// Resolve one reference. A legacy closure's bare paths keep their meaning: relative to the Loaf, or as written.
fn resolved_reference(
    reference: &str,
    anchored: bool,
    directory: &Path,
    anchors: &NativeAnchors,
) -> Result<PathBuf, String> {
    if !anchored {
        return Ok(located(directory, Path::new(reference)));
    }
    let Some((anchor, path)) = Anchor::parse(reference) else {
        return Err(format!("native closure reference `{reference}` names no anchor"));
    };
    let relative = Path::new(path);
    match anchor {
        Anchor::Sibling if relative.is_relative() => Ok(located(directory, relative)),
        Anchor::Toolchain if relative.is_relative() => {
            Ok(located(anchor_root(&anchors.toolchain, anchor, reference)?, relative))
        }
        Anchor::Bakery if relative.is_relative() => {
            Ok(located(anchor_root(&anchors.bakery, anchor, reference)?, relative))
        }
        Anchor::Absolute if relative.is_absolute() => Ok(relative.to_path_buf()),
        _ => Err(format!(
            "native closure reference `{reference}` has the wrong path form"
        )),
    }
}

/// The current home's root for `anchor`; a reference anchored at a root the compilation was not given is refused.
fn anchor_root<'a>(root: &'a Option<PathBuf>, anchor: Anchor, reference: &str) -> Result<&'a Path, String> {
    root.as_deref().ok_or_else(|| {
        format!(
            "native closure reference `{reference}` needs the current {} root",
            anchor.name()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closure published in one home resolves into another home that installed the same toolchain and shares the
    /// bakery, and its bytes do not depend on either home.
    #[test]
    fn closure_published_in_one_home_resolves_in_another() -> Result<(), String> {
        let home = |name: &str| NativeAnchors {
            toolchain: Some(PathBuf::from(format!("/homes/{name}/toolchains/t"))),
            bakery: Some(PathBuf::from("/shared/bakery")),
        };
        let first = home("a");
        let directory = Path::new("/shared/bakery/staging/unit");
        let closure = LoafNativeClosure {
            externs: vec![
                (
                    "runtime".into(),
                    PathBuf::from("/homes/a/toolchains/t/native/d/libruntime.rlib"),
                ),
                (
                    "dependency".into(),
                    PathBuf::from("/shared/bakery/libraries/l/libdependency.rlib"),
                ),
                ("system".into(), PathBuf::from("/opt/native/libsystem.rlib")),
            ],
            search: vec![PathBuf::from("/homes/a/toolchains/t/loaves/std/debug")],
            macro_externs: Vec::new(),
        };
        let published = closure.published(directory, &first)?;
        let text = serde_json::to_string(&published).map_err(|error| error.to_string())?;
        assert!(!text.contains("/homes/a"), "{text}");
        assert_eq!(published.schema.as_deref(), Some(NATIVE_CLOSURE_SCHEMA));
        assert_eq!(published.externs[0].1, "toolchain:native/d/libruntime.rlib");
        assert_eq!(published.externs[1].1, "bakery:libraries/l/libdependency.rlib");
        assert_eq!(published.externs[2].1, "absolute:/opt/native/libsystem.rlib");

        let library = Path::new("/shared/bakery/libraries/l");
        let read = LoafNativeClosure::resolved(published, library, &home("b"))?;
        assert_eq!(
            read.externs[0].1,
            PathBuf::from("/homes/b/toolchains/t/native/d/libruntime.rlib")
        );
        assert_eq!(
            read.externs[1].1,
            PathBuf::from("/shared/bakery/libraries/l/libdependency.rlib")
        );
        assert_eq!(read.externs[2].1, PathBuf::from("/opt/native/libsystem.rlib"));
        assert_eq!(
            read.search,
            vec![PathBuf::from("/homes/b/toolchains/t/loaves/std/debug")]
        );
        Ok(())
    }

    /// A legacy closure keeps its bare spellings, and an anchored one refuses a bare path or an unknown schema.
    #[test]
    fn legacy_closures_keep_bare_paths_and_anchored_ones_refuse_them() -> Result<(), String> {
        let directory = Path::new("/toolchain/loaves/std/debug");
        let legacy = PublishedNativeClosure {
            schema: None,
            externs: vec![("std".into(), "../../../native/d/libstd.rlib".into())],
            search: vec!["/opt/search".into()],
            macro_externs: Vec::new(),
        };
        let read = LoafNativeClosure::resolved(legacy.clone(), directory, &NativeAnchors::default())?;
        assert_eq!(read.externs[0].1, PathBuf::from("/toolchain/native/d/libstd.rlib"));
        assert_eq!(read.search, vec![PathBuf::from("/opt/search")]);
        let mut bare = legacy.clone();
        bare.schema = Some(NATIVE_CLOSURE_SCHEMA.into());
        assert!(LoafNativeClosure::resolved(bare, directory, &NativeAnchors::default()).is_err());
        let mut unknown = legacy;
        unknown.schema = Some("incan.native-closure/9".into());
        assert!(LoafNativeClosure::resolved(unknown, directory, &NativeAnchors::default()).is_err());
        Ok(())
    }
}
