# RFC 147: Result matching with `except`

- **Status:** Draft
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (Result and ordinary control flow)
    - RFC 029 (union types and narrowing)
    - RFC 070 (Result combinators)
    - RFC 071 (pattern alternatives)
    - RFC 094 (scoped resource lifetimes and finalization)
    - RFC 129 (binding and borrowing rules)
    - RFC 131 (typed errors, retained failures, and diagnostics)
    - RFC 134 (pattern bindings with exiting fallbacks)
    - RFC 135 (lazy Option defaults)
    - RFC 137 (named payload patterns)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Explore local typed Result matching through a single `except:` block containing ordinary match-style arms, independently of resource management. The working binding candidate makes the successful payload available to subsequent code while error arms supply a fallback value or explicitly leave the continuation. The pattern-block shape is the selected design direction; its attachment syntax, aggregate handling, and propagation boundaries remain under review. This RFC does not introduce thrown exceptions or panic catching.

## Motivation

A fallible operation often has a straightforward successful continuation but several distinct error policies: one error permits a default, another permits a different recovery, and the rest must propagate. A full match can push the continuation inside an arm. Combinators express many pipelines well, but substantial pattern-based recovery may require a separate callback whose only purpose is to house those branches.

The desired outcome is flat application code with visible typed recovery decisions and no loss of error identity or diagnostics. Resource acquisition and cleanup are separate concerns: a calculation, lookup, or boundary validation should not require a with statement merely because it returns Result. RFC 094 owns scoped resource lifetime and temporary-state restoration; this RFC owns the proposed local matching surface.

An additional retained failure must not erase another failure or make it unavailable for typed inspection. How local arms address several retained failures remains a central design question, not an incidental detail of the syntax.

## Goals

- Give local typed error recovery a readable pattern-based surface without requiring a resource scope.
- Reuse ordinary match arms, payload bindings, and pattern alternatives.
- Keep the successful continuation flat and statically typed.
- Preserve concrete error identities, payloads, hierarchy, origins, upstream relationships, and captured diagnostics under RFC 131.
- Compose with ordinary Result propagation and RFC 070 combinators without replacing them.
- Make evaluation, failure coverage, and recovery visible in compiler diagnostics and tooling.

## Non-Goals

- Adding exception throwing, changing raise, or catching panics.
- Defining resource entry, cleanup, or ownership transfer; those belong to RFC 094.
- Requiring with-attached handlers or making with a general Result wrapper.
- Turning Err into Option absence or extending RFC 135's or operator to Results.
- Replacing match, map_err, or_else, or RFC 134 exiting pattern bindings.
- Silently searching upstream causes, discarding unmatched failures, or treating a message string as error identity.
- Selecting a scope-outcome container, failures property, or as_result method from earlier test drives as a standard API.

## Guide-level explanation

All except examples describe proposed syntax. The application declarations below are supplied in full to make the test drive independent of an invented stdlib API. They use RFC 131's proposed error declarations. The binding attachment shown here is a candidate, not a finalized grammar or an implemented feature.

### Bind success or recover from a particular error

An application checks a requested batch size against a positive maximum:

```incan
error BatchError:
    InvalidRequest(requested: int)
    InvalidLimit(maximum: int)
    TooLarge(requested: int, maximum: int)


def checked_batch_size(requested: int, maximum: int) -> Result[int, BatchError]:
    if maximum <= 0:
        return Err(BatchError.InvalidLimit(maximum=maximum))
    if requested <= 0:
        return Err(BatchError.InvalidRequest(requested=requested))
    if requested > maximum:
        return Err(BatchError.TooLarge(requested=requested, maximum=maximum))
    return Ok(requested)
```

The application deliberately caps an oversized request but propagates invalid inputs:

```incan
def resolve_batch_size(requested: int, maximum: int) -> Result[int, BatchError]:
    size = checked_batch_size(requested, maximum)
    except:
        BatchError.TooLarge(maximum=limit) => limit
        Err(error) => return Err(error)

    return Ok(size)
```

Under this candidate, the successful Result payload binds size directly. TooLarge binds its maximum field to limit and supplies that integer as the replacement value. The final arm propagates the original unmatched error unchanged. These are expected test-drive outcomes, not executed compiler results:

- `resolve_batch_size(100, 256)` produces `Ok(100)` without running an error arm.
- `resolve_batch_size(1000, 256)` produces `Ok(256)` through the selected recovery.
- `resolve_batch_size(0, 256)` preserves InvalidRequest(requested=0).
- `resolve_batch_size(100, 0)` preserves InvalidLimit(maximum=0).

This differs from a fallback that always exits: the TooLarge arm establishes a replacement size and continues. RFC 134's let/else form deliberately requires its fallback to leave the continuation.

### Pure error conversion still uses map_err

When every failure should become one application-level error, a combinator remains direct:

```incan
error JobError:
    InvalidBatch(original: BatchError):
        caused_by = original


def job_batch_size(requested: int, maximum: int) -> Result[int, JobError]:
    return checked_batch_size(requested, maximum).map_err(JobError.InvalidBatch)
```

JobError and job_batch_size are application declarations. The error-constructor callback uses RFC 131's proposed constructor conversion ergonomics; the Result combinator itself is the RFC 070 surface. The wrapper retains the original typed error. This example converts failure; it does not cap the requested size or make the Result successful.

The proposed except form should earn its place where distinct patterns select distinct recovery behavior. A single unconditional conversion is not a reason to replace map_err with a larger block.

### Several retained failures remain an open test drive

Suppose one operation produces DataBaseError.ConnectionReset and its required cleanup also produces CleanupError.RollbackFailed. These names describe application error cases in this scenario; neither is a stdlib API declaration.

Both original failures must survive. A caller should remain able to inspect each by its concrete identity. The syntax does not yet establish whether arms match the complete failed Result, explicitly selected aggregate members, or a separate retained-error traversal. Matching one member must not silently handle its peers, and displaying one primary error must not discard the others.

A whole-aggregate catch-all was considered, but making a specific error arm stop being useful merely because another failure was retained raises a DX concern. Automatically running several ordinary-looking arms also has a cost: that would be traversal rather than ordinary one-value match dispatch, with additional questions about early return and remaining failures. Neither rule is selected. RFC 131 owns the shared retention and inspection contract; this RFC must settle how its local patterns consume that contract before advancing beyond Draft.

## Reference-level explanation

### Pattern-block direction

1. The local handling surface must use one except block containing ordinary `pattern =>` arms. Repeated except pattern clauses and an else error handler are not the working direction.
2. Typed patterns must resolve checked case identities, visibility, aliases, and payload fields under ordinary error and match rules. Patterns must not dispatch on rendered messages or invented stable string codes.
3. A bare error-case pattern abbreviates its corresponding Err pattern. For example, `BatchError.TooLarge(maximum=limit)` abbreviates `Err(BatchError.TooLarge(maximum=limit))`. This abbreviation must not search nested causes or erase a retained wrapper.
4. Err(error) binds the complete failure value presented to matching. The final aggregate presentation and per-member binding contract remain unresolved; a single-case test drive must not be presented as solving aggregate handling.
5. Successful completion must skip error arms. Handled and unhandled paths must remain statically checked; no path may continue with an uninitialized binding or silently declare an unmatched failure successful.

### Candidate binding contract

The guide's working form is a simple binding followed immediately by its handling block:

```incan
name = result_expression
except:
    error_pattern => fallback_expression
    Err(error) => return Err(error)
```

For this candidate, result_expression has type Result[T, E]. It must be evaluated exactly once. Ok(value) establishes the binding with type T, while a selected continuing error arm must supply a value assignable to T. An arm may instead leave the continuation through legal enclosing control flow. The source binding must not be available inside an arm before a value has been established.

This is a candidate contract for review, not a settled change to every assignment. A binding without an attached except block retains its ordinary Result-valued meaning. Host forms such as reassignment, destructuring, annotated bindings, return expressions, or standalone expressions have not been selected.

Arm ordering, coverage diagnostics, and the exact value-yielding rule for multiline arms must follow a precise selected contract before implementation. The natural candidate is ordinary ordered match dispatch for one value, with its existing unreachable-pattern and coverage checks; aggregate dispatch remains separately open.

### Propagation and diagnostics

The final design must identify which Result is matched and where fallible operations inside handlers propagate. It must not silently convert the handler into an ambient catch for unrelated question-mark propagation, acquisition failure, or panic. A fallback failure must not recursively re-enter its own handler through an implicit retry rule.

Unmatched and additional retained failures must remain available for propagation. The shared failure representation, diagnostics, and inspection follow RFC 131. Explicit recovery from a matched failure must be distinguishable from accidentally dropping another failure; this RFC does not create an unbounded global error-history buffer.

Diagnostics must identify the attached source expression, its Result success and error types, each selected case and payload binding, continuing-arm type mismatches, incomplete coverage, unreachable patterns, and any unresolved propagation or retention obligation. A misplaced question mark that has already extracted T must not be mistaken for a Result source merely because except follows it.

## Design details

### Interaction with other control flow

Result matching and resource lifetime are separate language concerns. RFC 094 remains responsible for running required cleanup before its operation is considered complete. A local Result handler must not bypass an active resource's required exit or change which typed failures that resource retains.

This proposal does not introduce a general try block. Handling an expression's returned Result and intercepting every failure originating anywhere in a suite are different contracts; the latter is not selected by the binding example. Ordinary return, loop control, and question-mark rules remain authoritative except for an explicitly specified new boundary. The exact boundary is listed as open below.

### Interaction with existing proposals

RFC 070 remains the standard Result combinator contract. RFC 134 handles successful pattern extraction whose unsuccessful path must exit. RFC 135 chooses among optional values rather than treating errors as absence. RFC 137 supplies named payload patterns. RFC 131 preserves original error identity, hierarchy, cause relationships, and aggregate members; this RFC must not invent a parallel error representation.

### Compatibility and inspectability

Existing Result bindings, match statements, combinators, and question-mark propagation must keep their current meaning when no new construct is present. The except spelling must be activated in its selected grammatical position without an unreviewed global identifier reservation. Older compilers may reject the proposed form; no source migration is required merely to retain existing explicit Result code.

Hover and signature help must expose both the source Result[T, E] and the candidate binding's resulting T, without suggesting that the source function itself became infallible. Go-to-definition, rename, diagnostics, and pattern completion must retain canonical case and field identities. Generated control flow must remain attributable to the source operation and selected arm, while original failure origins remain distinct from the recovery site.

Conformance requires paired success, recovered-error, and propagated-error examples; exactly-once evaluation; typed payload extraction; laziness of unselected arms; rejection of invalid continuing-arm types and unavailable bindings; and preservation of every remaining failure and its diagnostic provenance. Aggregate and propagation acceptance cases must be fixed before the RFC advances beyond Draft. No implementation or runtime proof is claimed by this document.

## Alternatives considered

- **Keep ordinary match and combinators only.** This remains a viable baseline. The new form must justify its grammar and typing costs through readable local recovery and a flatter continuation.
- **Repeated except clauses.** Not the preferred shape: a single block reuses ordinary match arms and their payload bindings and alternatives.
- **An else error block.** Less explicit about typed failure handling; the discussion selected except as the working keyword.
- **Attach error matching only to with.** Not the selected host direction: ordinary validation and calculations need no resource lifetime. Any future composition must preserve RFC 094's ownership and finalization contract.
- **Capture every with block in a new outcome container.** Experimental scope capture, failures views, and as_result methods exposed unresolved lifetime and multi-error semantics. This RFC does not select that API or require it for ordinary Result matching.
- **Automatically dispatch every retained failure through ordinary-looking arms.** Not selected: several arms could execute, and early return could leave peers unhandled. This needs an explicit traversal and retention contract, not an accidental change to match semantics.

## Drawbacks

- Local pattern recovery overlaps existing match and Result combinators, increasing the number of ways to express error handling.
- The binding candidate changes the visible bound type from Result[T, E] to T when an attached block is present; tooling and diagnostics must make that distinction clear.
- The except spelling may suggest thrown exceptions even though the proposed mechanism handles typed Result values only.
- Value-yielding multiline arms, propagation boundaries, and aggregate matching require real design work. Attractive examples with one error must not conceal those gaps.

## Layers affected

- **Parser / AST:** the chosen Result-handling host and one match-arm block require an explicit grammatical relationship.
- **Typechecker / control flow:** source Result typing, error patterns, binding availability, arm result types, coverage, propagation boundaries, and retained-error obligations must be checked together.
- **IR Lowering / Emission:** source evaluation, success binding, lazy handler dispatch, ordinary exits, and mandatory enclosing cleanup must preserve the selected semantics and error provenance.
- **Formatter:** attached blocks, match arms, and multiline bodies need stable formatting without conversion into repeated clauses or an else block.
- **LSP / Tooling:** completion, source Result and bound-value types, case and payload identity, diagnostics, and generated-control-flow explanations must reflect the selected contract.
- **Error integration:** RFC 131's shared retained-failure and diagnostic contracts must serve the new surface without a separate implicit error engine.

## Design Decisions

- Explore except as an independent Result-matching feature rather than making it part of resource management in RFC 094.
- Use one except block containing ordinary match-style arms, retaining typed case identity, payload bindings, and pattern alternatives. The attachment host is still a candidate.
- Preserve Result-based errors without changing raise, introducing exception throwing, or catching panics. Failure retention and diagnostic provenance follow RFC 131.

## Unresolved questions

- Which host syntax should be selected: the attached binding candidate, an independently usable expression form, or another explicit Result boundary?
- What value-yielding and definite-assignment rules govern multiline arms, reassignment, annotated bindings, and destructuring?
- Must error coverage be exhaustive, or may omitted cases propagate implicitly under a checked enclosing Result contract?
- Where do question-mark propagation, nested handler failures, explicit returns, and loop exits go, including fallible argument evaluation before the matched Result exists?
- How do arms address retained aggregate members or umbrellas while keeping every unmatched failure available, without implicitly searching causes or disguising multi-arm traversal as ordinary match?
- When recovery itself fails, how are the original failure and recovery failure retained and exposed through the shared RFC 131 contract?
- Does selective recovery provide enough readability benefit over ordinary match, or_else, and map_err to justify another construct? The complete batch-size example and retained-failure scenarios should decide this rather than syntax preference alone.

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
