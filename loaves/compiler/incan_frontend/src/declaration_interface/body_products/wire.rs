//! Admission and reconstruction of a single source module's private callable products.
//! Checker records and executable owners stay in their ordinary compiler-owned durable representations.

use super::*;
use crate::declaration_interface::callable_module::{CallableEntry, CallableLookupWitness};
use crate::library_manifest::CheckedExportReplay;
use crate::typechecker::callable_projection::{CallableHeader, CallableInventory, CallableOwner, CallableRead};
use crate::typechecker::{CheckedCellProjection, NamespaceCell};
use incan_semantics_core::{CompilerNodeId, executable_representation};

/// The driver supplies the ordinary Incan checker row codec to the already checked frontend.
pub trait CallableCheckerRows {
    fn write(&self, family: CallableRowFamily, fields: &[Vec<String>]) -> Result<Vec<u8>, String>;
    fn read(&self, family: CallableRowFamily, value: &[u8], owner: &str) -> Result<Vec<Vec<String>>, String>;
}

#[derive(Clone, Copy)]
pub enum CallableRowFamily {
    Declaration,
    Local,
    Final,
}

impl CallableHeader {
    fn from_header(header: &CheckedCallableInterface) -> Result<Self, String> {
        let CheckedCallableInterface {
            schema,
            identity,
            canonical,
            is_public,
            mutation,
            replay,
        } = header;
        if *schema != 1 {
            return Err("unsupported callable header schema".into());
        }
        let (package, version, source_path, replay_identity, function) =
            replay.callable_transport().map_err(|e| e.to_string())?;
        Ok(Self {
            identity: identity.clone(),
            canonical: canonical.clone(),
            is_public: *is_public,
            mutation: mutation.clone(),
            package,
            version,
            source_path,
            replay_identity,
            function,
        })
    }

    fn into_header(self) -> Result<CheckedCallableInterface, String> {
        let Self {
            identity,
            canonical,
            is_public,
            mutation,
            package,
            version,
            source_path,
            replay_identity,
            function,
        } = self;
        let replay =
            CheckedExportReplay::from_callable_transport(&package, &version, source_path, replay_identity, function)
                .map_err(|e| e.to_string())?;
        let header = CheckedCallableInterface {
            schema: 1,
            identity,
            canonical,
            is_public,
            mutation,
            replay,
        };
        header.validate().map_err(|e| e.to_string())?;
        Ok(header)
    }
}

impl CallableRead {
    fn from_guard(guard: &ReadGuard) -> Result<Self, String> {
        let ReadGuard {
            name,
            checked: CheckedCallableRead { witness, signatures },
            inputs,
        } = guard;
        if name != &witness.name {
            return Err("callable input and lookup name differ".into());
        }
        Ok(Self {
            name: name.clone(),
            origin: witness.origin.clone(),
            candidates: witness.candidates.clone(),
            signatures: signatures.clone(),
            inputs: inputs.clone(),
        })
    }

    fn into_guard(self) -> ReadGuard {
        let Self {
            name,
            origin,
            candidates,
            signatures,
            inputs,
        } = self;
        let witness = CallableLookupWitness {
            name: name.clone(),
            origin,
            candidates,
        };
        ReadGuard {
            name,
            checked: CheckedCallableRead { witness, signatures },
            inputs,
        }
    }
}

impl CallableOwner {
    fn from_owner(
        entry: &BodyProduct,
        module: &CompilerNodeId,
        source_path: &str,
        rows: &impl CallableCheckerRows,
    ) -> Result<Self, String> {
        let BodyProduct {
            header,
            fingerprints,
            header_source,
            source,
            reads,
            declaration,
            local,
            final_value,
            contexts,
            body,
        } = entry;
        if fingerprints.identity != header.identity {
            return Err("callable fingerprint belongs to a different header".into());
        }
        let identity = header.identity.render_compact();
        Ok(Self {
            header: CallableHeader::from_header(header)?,
            signature: fingerprints.signature.clone(),
            meaning: fingerprints.body.clone(),
            header_source: header_source.clone(),
            source: source.clone(),
            reads: reads.iter().map(CallableRead::from_guard).collect::<Result<_, _>>()?,
            declaration: rows.write(CallableRowFamily::Declaration, declaration)?,
            local: rows.write(CallableRowFamily::Local, local)?,
            final_value: rows.write(CallableRowFamily::Final, final_value)?,
            contexts: contexts
                .iter()
                .map(|(item, owner, ordinal)| ((item.clone(), owner.clone()), *ordinal))
                .collect(),
            body: executable_representation::encode_owner(module, source_path, &identity, body)
                .map_err(|e| e.to_string())?,
        })
    }

    fn into_owner(self, source_path: &str, rows: &impl CallableCheckerRows) -> Result<BodyProduct, String> {
        let Self {
            header,
            signature,
            meaning,
            header_source,
            source,
            reads,
            declaration,
            local,
            final_value,
            contexts,
            body,
        } = self;
        let header = header.into_header()?;
        let identity = header.identity.render_compact();
        let executable = executable_representation::decode_owner(&body).map_err(|e| e.to_string())?;
        if executable.identity != identity
            || executable.source != source_path
            || executable.body.canonical.as_ref() != Some(&header.canonical)
        {
            return Err("callable owner frame, source and header disagree".into());
        }
        Ok(BodyProduct {
            fingerprints: CheckedDeclarationFingerprints {
                identity: header.identity.clone(),
                signature,
                body: meaning,
            },
            header,
            header_source,
            source,
            reads: reads.into_iter().map(CallableRead::into_guard).collect(),
            declaration: rows.read(CallableRowFamily::Declaration, &declaration, &identity)?,
            local: rows.read(CallableRowFamily::Local, &local, &identity)?,
            final_value: rows.read(CallableRowFamily::Final, &final_value, &identity)?,
            contexts: contexts
                .into_iter()
                .map(|((item, owner), ordinal)| (item, owner, ordinal))
                .collect(),
            body: executable.body,
        })
    }
}

impl CallableBodyProducts {
    pub fn crumb_fields(
        &self,
        module: &CompilerNodeId,
        source_path: &str,
        rows: &impl CallableCheckerRows,
    ) -> Result<NamespaceCell, String> {
        self.validate().map_err(|e| e.to_string())?;
        let Self {
            namespace,
            environment,
            interface,
            entries,
        } = self;
        if interface.schema != 1 {
            return Err("unsupported checked callable module schema".into());
        }
        let callables = interface
            .callables
            .iter()
            .map(|entry| Ok((CallableHeader::from_header(&entry.header)?, entry.signature.clone())))
            .collect::<Result<_, String>>()?;
        Ok(CallableInventory {
            namespace: namespace.clone(),
            environment: environment.clone(),
            origin: interface.origin.clone(),
            callables,
            entries: entries
                .iter()
                .map(|e| CallableOwner::from_owner(e, module, source_path, rows))
                .collect::<Result<_, _>>()?,
        }
        .project())
    }

    pub fn from_crumb_fields(
        root: NamespaceCell,
        source_path: &str,
        rows: &impl CallableCheckerRows,
    ) -> Result<Self, String> {
        let CallableInventory {
            namespace,
            environment,
            origin,
            callables,
            entries,
        } = CallableInventory::restore(root)?;
        let callables = callables
            .into_iter()
            .map(|(header, signature)| {
                Ok(CallableEntry {
                    header: header.into_header()?,
                    signature,
                })
            })
            .collect::<Result<_, String>>()?;
        let result = Self {
            namespace,
            environment,
            interface: CheckedCallableModule {
                schema: 1,
                origin,
                callables,
            },
            entries: entries
                .into_iter()
                .map(|e| e.into_owner(source_path, rows))
                .collect::<Result<_, _>>()?,
        };
        result.validate().map_err(|e| e.to_string())?;
        Ok(result)
    }
}
