//! Surface index validation and decoding, and the streaming surface reader.

use super::*;

impl SurfaceIndex {
    /// Validate duplicate-free canonical ownership, contiguous disjoint ranges, and payload bounds before selection.
    pub fn validate(&self, payload_length: u64) -> Result<(), ExecutableRepresentationError> {
        if self
            .implementation
            .keys()
            .any(|identity| self.declarations.contains_key(identity))
        {
            return Err(malformed("public and implementation identities overlap"));
        }
        let mut next_offset = 0;
        let entries: BTreeMap<_, _> = self.declarations.iter().chain(&self.implementation).collect();
        for (identity, coverage) in entries {
            if identity.scope_discriminant.is_some()
                || !matches!(&identity.origin, crate::SymbolOrigin::Package { library, .. } if library == &self.library)
            {
                return Err(malformed("index identity does not belong to the declaring package"));
            }
            if let DeclarationCoverage::TypeContext { owner } = coverage
                && (owner.origin != identity.origin
                    || !(matches!(
                        (&identity.kind, &owner.kind),
                        (
                            crate::SemanticSourceTargetKind::Field,
                            crate::SemanticSourceTargetKind::Model | crate::SemanticSourceTargetKind::Class
                        ) | (
                            crate::SemanticSourceTargetKind::Variant,
                            crate::SemanticSourceTargetKind::Enum
                        ) | (
                            crate::SemanticSourceTargetKind::Method,
                            crate::SemanticSourceTargetKind::Trait
                        )
                    ) || publication::is_generated_value_enum_member(identity, owner))
                    || !matches!(self.execution_coverage(owner), Ok(DeclarationCoverage::Covered { .. })))
            {
                return Err(malformed(
                    "public member context does not select a covered declaring type",
                ));
            }
            if let DeclarationCoverage::Covered {
                offset,
                length,
                requirements,
            } = coverage
            {
                if *offset != next_offset || *length == 0 {
                    return Err(malformed("fragment ranges overlap, contain gaps, or are empty"));
                }
                next_offset = offset
                    .checked_add(*length)
                    .ok_or_else(|| malformed("fragment range overflow"))?;
                if requirements.windows(2).any(|pair| pair[0] >= pair[1]) {
                    return Err(malformed("requirements are duplicated or unsorted"));
                }
            }
        }
        if next_offset != payload_length {
            return Err(malformed("index does not match payload length"));
        }
        Ok(())
    }

    /// Decode a bounded index. The caller must already have accepted the stable envelope version.
    pub fn decode(bytes: &[u8], payload_length: u64) -> Result<Self, ExecutableRepresentationError> {
        let index: Self = postcard::from_bytes(bytes).map_err(|error| malformed(error.to_string()))?;
        // Re-encoding checks map ordering and duplicates, which serde's BTreeMap decoder otherwise normalizes.
        if postcard::to_allocvec(&index).map_err(|error| malformed(error.to_string()))? != bytes {
            return Err(malformed("noncanonical or duplicate index entries"));
        }
        index.validate(payload_length)?;
        Ok(index)
    }

    /// Inspect explicit coverage before any payload is decoded.
    pub fn coverage(
        &self,
        identity: &CanonicalSymbolId,
    ) -> Result<&DeclarationCoverage, ExecutableRepresentationError> {
        self.declarations
            .get(identity)
            .ok_or_else(|| ExecutableRepresentationError::DeclarationNotCovered {
                declaration: identity.declaration_name.clone(),
            })
    }

    /// Inspect a dependency reached through a selected fragment; callers must separately authorize public roots.
    pub fn execution_coverage(
        &self,
        identity: &CanonicalSymbolId,
    ) -> Result<&DeclarationCoverage, ExecutableRepresentationError> {
        self.declarations
            .get(identity)
            .or_else(|| self.implementation.get(identity))
            .ok_or_else(|| ExecutableRepresentationError::DeclarationNotCovered {
                declaration: identity.declaration_name.clone(),
            })
    }

    /// Decode exactly the selected range and bind its payload identity back to the index and manifest identity.
    pub fn decode_declaration(
        &self,
        identity: &CanonicalSymbolId,
        bytes: &[u8],
    ) -> Result<ExecutableDeclaration, ExecutableRepresentationError> {
        let DeclarationCoverage::Covered { length, .. } = self.execution_coverage(identity)? else {
            return Err(ExecutableRepresentationError::DeclarationNotCovered {
                declaration: identity.declaration_name.clone(),
            });
        };
        if u64::try_from(bytes.len()).ok() != Some(*length) {
            return Err(malformed("selected fragment length differs from its index"));
        }
        let (mut declaration, remainder): (ExecutableDeclaration, _) =
            postcard::take_from_bytes(bytes).map_err(|error| malformed(error.to_string()))?;
        match &mut declaration {
            ExecutableDeclaration::Nominal(nominal) | ExecutableDeclaration::NominalWithTraits(nominal, _) => {
                restore_trait_fields(std::slice::from_mut(nominal), remainder)?;
            }
            _ if !remainder.is_empty() => return Err(malformed("unexpected declaration trailer")),
            _ => {}
        }
        if declaration.identity() != Some(identity) {
            return Err(malformed("fragment identity differs from its index"));
        }
        Ok(declaration)
    }
}

impl<'bytes> SurfaceReader<'bytes> {
    /// Refuse the stable prefix first, then decode only the bounded metadata index.
    pub fn open(bytes: &'bytes [u8]) -> Result<Self, ExecutableRepresentationError> {
        require_supported_version(bytes)?;
        let (_, remainder) = postcard::take_from_bytes::<u32>(bytes).map_err(|error| malformed(error.to_string()))?;
        let length_bytes: [u8; 8] = remainder
            .get(..8)
            .ok_or_else(|| malformed("missing index length"))?
            .try_into()
            .map_err(|_| malformed("invalid index length"))?;
        let declared_index_length = u64::from_le_bytes(length_bytes);
        if declared_index_length > MAX_SURFACE_INDEX_BYTES {
            return Err(malformed("index length exceeds its bounded envelope"));
        }
        let index_length = usize::try_from(declared_index_length).map_err(|_| malformed("index too large"))?;
        let end = 8usize
            .checked_add(index_length)
            .ok_or_else(|| malformed("index length overflow"))?;
        let encoded_index = remainder
            .get(8..end)
            .ok_or_else(|| malformed("index extends beyond representation"))?;
        let payload = remainder.get(end..).ok_or_else(|| malformed("missing payload"))?;
        let index = SurfaceIndex::decode(
            encoded_index,
            u64::try_from(payload.len()).map_err(|_| malformed("payload too large"))?,
        )?;
        Ok(Self { index, payload })
    }

    /// Declared package coverage, inspectable without decoding executable payloads.
    pub fn index(&self) -> &SurfaceIndex {
        &self.index
    }

    /// Canonical identities with covered declaration fragments; uncovered exports remain visible through the index.
    pub fn covered_identities(&self) -> impl Iterator<Item = &CanonicalSymbolId> {
        self.index.declarations.iter().filter_map(|(identity, coverage)| {
            matches!(coverage, DeclarationCoverage::Covered { .. }).then_some(identity)
        })
    }

    /// Whether the index explicitly covers this identity.
    pub fn covers(&self, identity: &CanonicalSymbolId) -> bool {
        matches!(
            self.index.declarations.get(identity),
            Some(DeclarationCoverage::Covered { .. })
        )
    }

    /// Decode one public body. A type fragment is never treated as a callable.
    pub fn declaration(&self, identity: &CanonicalSymbolId) -> Result<Body, ExecutableRepresentationError> {
        match self.fragment(identity)? {
            ExecutableDeclaration::Body(body)
            | ExecutableDeclaration::NativeBody { body, .. }
            | ExecutableDeclaration::DecoratedBody { body, .. }
            | ExecutableDeclaration::GenericDecoratedBody { body, .. }
            | ExecutableDeclaration::InvocationDecoratedBody { body, .. } => Ok(*body),
            _ => Err(malformed(
                "selected declaration is type context, not an executable body",
            )),
        }
    }

    /// Decode only one indexed fragment, preserving unrelated payloads as uninterpreted bytes.
    pub fn fragment(
        &self,
        identity: &CanonicalSymbolId,
    ) -> Result<ExecutableDeclaration, ExecutableRepresentationError> {
        let DeclarationCoverage::Covered { offset, length, .. } = self.index.coverage(identity)? else {
            return Err(ExecutableRepresentationError::DeclarationNotCovered {
                declaration: identity.declaration_name.clone(),
            });
        };
        let start = usize::try_from(*offset).map_err(|_| malformed("fragment offset exceeds host range"))?;
        let length = usize::try_from(*length).map_err(|_| malformed("fragment length exceeds host range"))?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| malformed("fragment range overflow"))?;
        self.index.decode_declaration(
            identity,
            self.payload
                .get(start..end)
                .ok_or_else(|| malformed("fragment outside payload"))?,
        )
    }
}
