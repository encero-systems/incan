# Implementing the typed-value protocol

Status: implementation brief for [RFC 173](../../RFCs/173_walking_and_reconstructing_typed_values.md), tracked by [#2519](https://github.com/encero-systems/incan/issues/2519). This is a plan, not a claim that the hooks have shipped. It does not change the RFC or specify COMIC syntax or Crumb bytes.

## Outcome

A library outside the compiler can walk and reconstruct a model declared in another Loaf, including private stored fields, through ordinary checked trait calls. Nested models, payload enums, newtypes and containers use that same contract. Reconstruction restores stored state and enforces final validation without replaying construction transformations. A handwritten class implementation participates through the same bounds. No codec name, wire tag or hash algorithm belongs in the compiler.

The first acceptance program uses a generated provider model, an independently packaged recording walker and an independently packaged typed source. It builds a fresh consumer without the provider's source, walks the value, restores it and executes native code. Handwritten substitutes for generated hooks do not satisfy this acceptance program.

## Prerequisites and current evidence

The implementation base must include the bounded static dispatch fix for #2714, the generic trait self-bound fix for #2774 and the RFC 173 change that allows `Self` as a call head. The latter two have source changes but need qualified-toolchain proof before their presence is called working support. Recheck the nested generic reconstruction case after #2762; do not assume that inference through `S with Source[E]` has been fixed by a neighboring change.

The existing manual protocol probes establish useful controls, not automatic derivation: private-field reconstruction, a hand-enforced final validation boundary and refusal of an escaping borrowed backing have executed. Concrete overrides were needed in other probes. Those successes do not establish that a default method can recurse through the generic protocol, that validation is generated, or that generated members survive a compiled-Loaf boundary.

The checked field metadata already includes identity, canonical participation and rename history on the queued implementation base. Extend that metadata; do not recompute identities from a display name inside a walker. The existing `__fields__()` contract is a public inspection view. It is not the complete stored shape, and its visibility policy must not silently change.

## Pattern intake and ownership

Use these same-stage precedents on the implementation base:

| Owner | Existing precedent | Change owned here |
| --- | --- | --- |
| Primitive reflection | `loaves/stdlib/primitives/src/reflection.incn` and its existing facade | Incan declarations of Walk, Reconstruct, Walker, Source, ShapeInfo and required metadata; facade re-exports preserve one identity. |
| Checked declarations and bounds | Frontend collection, derive validation and the #2774 self-bound change | Register automatic protocol availability, resolve concrete field types, check custom replacement completeness and diagnose unsupported use paths. |
| Checked callable records | #2714 and RFC 173 Self-call changes | Preserve selected trait member, provider identity and concrete generic substitutions in local and published calls. |
| Generated operations | `loaves/compiler/incan_mir_lowering/src/reflection.incn` and `derived_methods.incn` | Follow retained checked identities; lower typed calls and field borrows through existing ownership rules. Never dispatch on a bare member name. |
| Reconstruction | `loaves/compiler/incan_mir_lowering/src/model_construction.incn` and checked construction contracts | Restore already-decoded stored fields through a distinct model-owned operation; invoke final validation without entering the ordinary input path. |
| Cross-Loaf proof | `workspaces/contracts/fixtures/project/dependencies/compiled_generic_bounds` | Exercise the generated protocol across a real published provider and fresh consumer. |

These are intake locations, not permission to put the entire feature into their existing files. Split new modules by concern. Read their current source before editing: the stdlib ownership and frontend contracts are evolving.

Author protocol behavior in Incan. Existing Rust frontend plumbing can be changed only where that compiler boundary owns the requirement; record the exact tracked language/compiler limitation that makes new Rust necessary. Do not add a Rust codec, dynamic reflection bridge or format-specific generated visitor for convenience.

## Delivery sequence

### 1. Prove the language prerequisites

Run the four original minimal failures against the selected qualified bundle. Record source and bundle pins separately. Then run the generated-member version of each relevant case: `Self.__reconstruct__`, bounded `T.__reconstruct__`, a Walker default passing `self`, and nested reconstruction whose error parameter is inferred through the Source bound. If one still fails, reduce it and link its owning issue. Continue independent protocol declarations and test preparation; do not mark the whole RFC blocked by one call shape.

### 2. Declare one protocol in primitives

Declare the RFC's traits and carriers in the primitive reflection package and re-export them from the existing public reflection surface. Resolve both import paths to the same declarations. Specify all scalar and structural methods, including default structural recursion, sequence/container end checks and Source's typed failure and collection-policy methods. Use concrete checked type parameters, not an erased dynamic value tree.

Keep typed `Source.field[V] -> Result[Option[V], E]` intact. Its outer Option indicates whether the input supplied the field. A supplied `V=Option[int]` may itself be None; a failed read is a third state. Source implementations own their input positions, budgets and errors. A generated hook must retain those through nested calls.

### 3. Generate complete shapes and on-demand availability

Generate ShapeInfo for every concrete supported model, enum and newtype. Retain all stored fields, their checked identities and canonical flags, declared types, declaration positions, empty-default facts and rename/lifecycle metadata. Include private fields and exclude virtual construction inputs. Generate finite references for recursive shapes. The shape operation does not execute a default factory or construction code.

The checker admits an otherwise ordinary declaration even if a field is a runtime resource. A reached Walk/Reconstruct call or bound diagnoses the full unsupported field path and the owning field declaration. Generic support is checked at each closed instantiation; accepting `Box[int]` must not imply accepting `Box[LiveHandle]`. A class does not get automatic members. Secret types never gain the protocol, including through nested containers or a custom opt-in.

A complete handwritten replacement is checked against the same traits. Partial replacement of the generated contract is diagnosed under the RFC; it must not combine a custom stored form with an incompatible generated shape. Verify custom implementation and automatic implementation through the same generic consumer.

### 4. Generate typed walks

Emit begin/field/end operations in ascending member-identity order, independently of source order. Enums select their variant before visiting its payload. Newtypes walk the underlying value. Scalars and containers call the primitive protocol operations; Option layers and union selections remain explicit. A default walker can descend without a concrete override for every model.

Borrow each field through normal checked ownership. Do not clone a field, take ownership of its container, materialize all field values, or serialize it to text merely to visit it. The walker may allocate its own result deliberately; measure that separately. A walker storing an invalid reference beyond the value's backing lifetime must still be refused.

### 5. Generate reconstruction and final validation

Read every stored field by checked identity and full type. Retain per-field availability separately from the field's value. Supply absence only through the format's explicit policy. A current nonempty constructor default does not repair missing stored data. A changed default never replaces a supplied value. Construct no publicly observable partial instance.

Restore the aggregate through the model-owned operation, then execute the applicable final field and model validation. Newtypes enforce their constraints. Send the original ValidationError through `source.invalid`, preserving its hierarchy and path. An implementation cannot bypass final validation because a stored hash matches or because the source claims the value was validated earlier.

The final-state validation operation must be shared with normal construction's final step; the entire normal constructor must not be reused. A counter on construction normalization must remain unchanged during restoration. Validators may observe restored state; construction fieldmethods and virtual inputs must not run.

Honor `collects()` and `collected()`: continue independently decodable fields when requested, retain each failure once and run only validation whose input dependencies are available. A failed optional read is not absence. A structural failure that prevents continuation reports honest coverage. No policy returns a partially restored model.

### 6. Publish and consume generated contracts

Publish the same checked generated-member facts and bodies needed by an ordinary consumer. Include complete shape metadata, identity and rename continuity, concrete generic bindings, trait witnesses and the final-validation obligation. A consumer cannot reconstruct private storage from public field reflection alone.

Exercise imports, re-exports, a provider-owned nested type, generic instantiations and a consumer-owned walker/source. Remove access to the provider source in the consumer proof. A same-spelled user trait or method must not acquire privileged generated behavior. Corrupt or missing retained facts are refused, not repaired from a guessed public path.

## Required red cases and their pass conditions

Add narrowly failing contract fixtures before implementation. Existing manual probes are controls, not replacements for these generated cases.

| Case | Exact observable result |
| --- | --- |
| Private state | A public provider model retains its private stored ID through a foreign source/walker. Ordinary external private-field construction and assignment still fail. |
| Field order | Reverse declaration order and retain the same recorded identity-order walk; a rename preserves its earliest identity. A collision fails before values are visited. |
| Stateful default walker | Override only the reference method, walk a nested list/model/enum and collect every reference once with no concrete per-model override. |
| Stored minutes | Construct from virtual hours=2 and store minutes=120. Restore minutes=120, run final validation once and do not invoke the hours fieldmethod. Restoring -120 fails the original validation. |
| Input normalization | A field-backed normalization counter changes on ordinary input, not on restoration. Stored normalized data does not normalize twice. |
| Missing versus None | Distinguish missing, present None, Some(None), Some(Some(7)) and a failed field read. Test both a list element and model field. |
| Absent policy | Missing empty collection succeeds only when the source permits its declared empty default. Missing required field with a nonempty default fails. A supplied value survives a default change. |
| CollectAll | Two independently bad fields yield two located failures once; a check requiring a failed field is reported blocked, while an independent final check may run. No partial model escapes. |
| FailFast | Stop at the first field failure under the selected policy; do not reset that policy at a nested model. |
| Generic shapes | Generated `Box[int]` and `Box[str]` have concrete field types. Nested Source error inference works without casts or concrete overrides. |
| Enum and containers | Payload enum, Result, list, tuple, dict, set, frozen forms, union and constrained newtype round trip; invalid variant, arity and numeric range are refused with paths. |
| Source completion | Extra data inside a selected container fails its end operation; the next framed record is not mistaken for trailing bytes. |
| Shared budgets | Deep nesting consumes one source budget; recursion into a new model cannot reset it. A retained byte view cannot outlive its backing. |
| Unsupported use | A model containing a live resource can be declared but a reached call/bound fails with the full field path. Direct and nested secrets are refused without suggesting an opt-in. |
| Custom class | Complete handwritten Walk/Reconstruct/shape works through the same generic calls; incomplete replacement fails with a specific diagnostic. |
| Compiled provider | Fresh source-absent consumer performs the preceding private/generic/validation cases using generated provider members and native execution. |
| Cost | Unreached hooks emit no reachable bodies. Walking a large list field adds no implicit field clone; report output allocations separately. |

## Validation and handoff

Use the repository's contract runner and its checked expectations. The documented entry is:

```text
incan run workspaces/contracts/src/main.incn -- INCAN workspaces/contracts/fixtures --scratch DIR --jobs N SELECTOR...
```

Replace INCAN, DIR, N and SELECTOR with pinned executable, isolated scratch, admitted worker budget and the actual registered cases. Run focused cases first; run all eight contract families with the matching language server before claiming integration readiness. Compare the complete PASS-key set with the base, not just the total. The qualification owner runs the final release proof after the queued prerequisites are qualified. A successful check-only run is not native execution proof.

| Gate | Evidence required |
| --- | --- |
| Red/green | Exact fixture selectors, full first diagnostic, subsequent native output and exit status. |
| Boundary proof | Provider package pins, source-absent consumer receipt, generic/default-member calls and refusal controls. |
| Ownership | No implicit field clones; retained-view escape refusal; shared resource budgets. |
| Regression | Full contract census with matching LSP and no lost baseline PASS keys. |
| Source quality | Changed Incan is readable and documented; changed Rust boundary functions have rustdocs; existing format, prose and path gates pass. |
| Documentation | Public protocol reference reflects the implemented subset and exact imports. No serializer availability is implied by hook support. |
| Lifecycle | Keep RFC 173 Planned or In Progress until its full conformance list has implementation evidence. A narrow generated-model success does not close #2519. |

The handoff identifies source commit, toolchain digest, code changes, test selectors and receipts, and every remaining RFC conformance item. Codec parser/printer, hash preimage, binary layout, database transactions and compiler-record migration are later consumers of this protocol, not implicit deliverables of hook generation.
