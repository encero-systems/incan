//! Compare a complete private snapshot incrementally, retaining one module or Rust item instead of a second program.

use std::fmt;
use std::io::Read;

use incan_frontend::library_manifest::LibraryManifest;
use incan_lang::interop::RustItemMetadata;
use incan_semantics_core::body_ir::BodyIrModule;
use serde::Deserializer;
use serde::de::{DeserializeOwned, DeserializeSeed, Error as _, SeqAccess, Visitor};

use super::{CheckedProgram, StoredModule, restore_module};

/// Validate every encoded field against its checked owner, with bounded temporary destination allocations.
pub(super) fn verify<R: Read>(
    program: &mut CheckedProgram,
    reader: R,
    scratch: &mut [u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let flavor = postcard::de_flavors::io::io::IOReader::new(reader, scratch);
    let mut decoder = postcard::Deserializer::from_flavor(flavor);
    ProgramWitness(program).deserialize(&mut decoder)?;
    let (mut reader, _) = decoder.finalize()?;
    if reader.read(&mut [0])? != 0 {
        return Err("trailing checked-source facts".into());
    }
    Ok(())
}

/// The original program remains the authority while its snapshot is visited in positional field order.
struct ProgramWitness<'a>(&'a mut CheckedProgram);

impl<'a, 'de> DeserializeSeed<'de> for ProgramWitness<'a> {
    type Value = ();

    /// Read the exact seven fields of the compiler-bound private program representation.
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        decoder.deserialize_tuple(7, self)
    }
}

impl<'de> Visitor<'de> for ProgramWitness<'_> {
    type Value = ();

    /// Describe the private format when serde refuses an incompatible destination.
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the complete checked-source program")
    }

    /// Compare every source, publication and provenance field; large inventories validate one record at a time.
    fn visit_seq<A: SeqAccess<'de>>(self, mut input: A) -> Result<(), A::Error> {
        let original = self.0;

        // ---- Checked declarations and publication ----
        input
            .next_element_seed(ModulesWitness(&original.modules))?
            .ok_or_else(missing)?;
        let manifest: Option<String> = input.next_element()?.ok_or_else(missing)?;
        if !manifest_matches(original, manifest).map_err(A::Error::custom)? {
            return Err(A::Error::custom("checked manifest round trip lost a fact"));
        }
        let rust = original
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.rust_abi.as_ref())
            .map(|abi| abi.items.as_slice());
        input
            .next_element_seed(OptionalRustWitness(rust))?
            .ok_or_else(missing)?;

        // ---- Authored source and entry identity ----
        input
            .next_element_seed(ValuesWitness(&original.sources))?
            .ok_or_else(missing)?;
        input
            .next_element_seed(ValuesWitness(&original.files))?
            .ok_or_else(missing)?;
        let entry: usize = input.next_element()?.ok_or_else(missing)?;
        let own: usize = input.next_element()?.ok_or_else(missing)?;
        if entry != original.entry || own != original.own {
            return Err(A::Error::custom("checked entry inventory differs"));
        }

        Ok(())
    }
}

/// Compare the JSON-shaped manifest while its separately encoded Rust inventory is temporarily detached.
fn manifest_matches(program: &mut CheckedProgram, header: Option<String>) -> Result<bool, String> {
    let decoded = header
        .as_deref()
        .map(LibraryManifest::from_json_str)
        .transpose()
        .map_err(|error| error.to_string())?;
    match (program.manifest.as_mut(), decoded) {
        (Some(original), Some(decoded)) => {
            let items = original.rust_abi.as_mut().map(|abi| std::mem::take(&mut abi.items));
            let same = *original == decoded;
            if let (Some(abi), Some(items)) = (original.rust_abi.as_mut(), items) {
                abi.items = items;
            }
            Ok(same)
        }
        (None, None) => Ok(true),
        _ => Ok(false),
    }
}

/// One restored module is released before the next module allocates its destination facts.
struct ModulesWitness<'a>(&'a [BodyIrModule]);

impl<'a, 'de> DeserializeSeed<'de> for ModulesWitness<'a> {
    type Value = ();

    /// Preserve postcard's ordinary sequence framing instead of interpreting its length independently.
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        decoder.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for ModulesWitness<'_> {
    type Value = ();

    /// Name the inventory whose structural round trip must remain complete.
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the checked modules in their original order")
    }

    /// Restore all skipped fields before comparing a module, and refuse truncated or additional records.
    fn visit_seq<A: SeqAccess<'de>>(self, mut input: A) -> Result<(), A::Error> {
        for original in self.0 {
            let stored: StoredModule = input.next_element()?.ok_or_else(missing)?;
            let restored = restore_module(stored).map_err(|error| A::Error::custom(error.to_string()))?;
            if original != &restored {
                return Err(A::Error::custom("checked module round trip lost a fact"));
            }
        }
        if input.next_element::<StoredModule>()?.is_some() {
            return Err(A::Error::custom("additional checked module"));
        }
        Ok(())
    }
}

/// Ordered source strings and native metadata use their existing owned serde forms, one element at a time.
struct ValuesWitness<'a, T>(&'a [T]);

impl<'a, 'de, T: DeserializeOwned + PartialEq> DeserializeSeed<'de> for ValuesWitness<'a, T> {
    type Value = ();

    /// Visit the same sequence shape the borrowed program serializer emits.
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        decoder.deserialize_seq(self)
    }
}

impl<'de, T: DeserializeOwned + PartialEq> Visitor<'de> for ValuesWitness<'_, T> {
    type Value = ();

    /// Describe a sequence bound to the original program's exact values.
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the original checked inventory")
    }

    /// Compare each complete value before releasing it, retaining neither a parallel table nor a second program.
    fn visit_seq<A: SeqAccess<'de>>(self, mut input: A) -> Result<(), A::Error> {
        for original in self.0 {
            let restored: T = input.next_element()?.ok_or_else(missing)?;
            if original != &restored {
                return Err(A::Error::custom("checked value round trip lost a fact"));
            }
        }
        if input.next_element::<T>()?.is_some() {
            return Err(A::Error::custom("additional checked value"));
        }
        Ok(())
    }
}

/// Preserve absent versus present-empty native inventories without allocating their complete destination vector.
struct OptionalRustWitness<'a>(Option<&'a [RustItemMetadata]>);

impl<'a, 'de> DeserializeSeed<'de> for OptionalRustWitness<'a> {
    type Value = ();

    /// Retain serde's option tag, independent of the ABI inventory's element count.
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        decoder.deserialize_option(self)
    }
}

impl<'de> Visitor<'de> for OptionalRustWitness<'_> {
    type Value = ();

    /// Describe the distinction between no Rust metadata and an admitted empty inventory.
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the original optional Rust inventory")
    }

    /// Accept absence only when the source program also has no Rust ABI payload.
    fn visit_none<E: serde::de::Error>(self) -> Result<(), E> {
        if self.0.is_some() {
            return Err(E::custom("missing checked Rust inventory"));
        }
        Ok(())
    }

    /// Validate the complete native inventory through its existing binary element codec.
    fn visit_some<D: Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        let original = self
            .0
            .ok_or_else(|| D::Error::custom("additional checked Rust inventory"))?;
        ValuesWitness(original).deserialize(decoder)
    }
}

/// Report a missing private field consistently without any infallible parser or panic path.
fn missing<E: serde::de::Error>() -> E {
    E::custom("truncated checked-source inventory")
}
