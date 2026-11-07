# RFC 147: Result matching with `except`

- **Status:** Planned
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (Result and ordinary control flow)
    - RFC 029 (union types and narrowing)
    - RFC 070 (Result combinators)
    - RFC 071 (pattern alternatives)
    - RFC 094 (scoped resource lifetimes and finalization)
    - RFC 129 (binding rules)
    - RFC 131 (typed errors, retained failures, and diagnostics)
    - RFC 134 (pattern bindings with exiting fallbacks)
    - RFC 135 (lazy Option defaults)
    - RFC 137 (named payload patterns)
    - RFC 148 (must-use values and deliberate discard)
    - RFC 149 (non-exhaustive families)
    - RFC 164 (explicit ownership and borrowing)
- **Issue:** [#2650](https://github.com/encero-systems/incan/issues/2650)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

A statement ending in `?` may be followed by an `except:` block of ordinary match arms, which handles the listed errors where they happen and lets every other error go to the caller as `?` always does. It reads as "`?`, except for these cases". An arm either supplies a replacement value, so the function continues, or leaves through `return`, `break` or `continue`. This is not exception handling: nothing is thrown, `raise` is unchanged, and panics are not caught. It is a shorter way to match on one `Result` value.

## Core model

1. **`?` still means `?`.** The value is the success payload and errors go to the caller, with the same checks as anywhere else.
2. **The block lists the exceptions.** Its arms see the error, and an error no arm handles propagates.
3. **One value, one match.** The block matches the `Result` of its own line and nothing else, under the ordinary rules for match arms.
4. **An arm continues or leaves.** A value replaces the payload, and an exit leaves as it would anywhere else.
5. **Removing the block leaves working code.** The line above it is an ordinary `?` line.

## Motivation

A function calls something that returns `Result[T, E]`. The success path is the main story, one or two errors have a sensible local fix, and every other error should go to the caller. Three things exist today, and none is ideal:

- `?` propagates every error, so it cannot handle just one.
- Combinators such as `or_else` put case-by-case logic inside a callback whose only job is to hold those branches.
- `match` works, but the success value has to come out through an `Ok(value) => value` arm, every error arm has to spell out `Err(...)`, and propagating the rest has to be written by hand:

```incan
profile = match store.fetch(user_id):
    Ok(value) => value
    Err(StoreError.Unavailable(_)) => cache.last_known(user_id)
    Err(error) => return Err(error)
```

Only one line of that says anything specific to this program.

## Goals

- Handle selected errors locally while keeping the successful path flat.
- Keep propagation visible at the call, with the rules `?` already has.
- Reuse ordinary match arms, payload bindings, guards and alternatives.
- Let a function return a narrower error set than the call it handles.
- Preserve error identity, payloads and diagnostics under RFC 131.

## Non-Goals

- Throwing exceptions, changing `raise`, or catching panics.
- A general `try` block that intercepts every failure in a suite.
- Resource entry and cleanup, which belong to RFC 094.
- Replacing `match`, `map_err`, `or_else`, or RFC 134's `let ... else`.
- Searching upstream causes, or matching on message text.

## Guide-level explanation

Every `except` example below is proposed by this RFC.

### Handle one error and let the rest go

A profile page serves a cached copy while the store is down. Every other store failure goes to the caller:

```incan
error StoreError:
    Unavailable(reason: str)
    Forbidden(user_id: str)
    Corrupt(path: str)

def profile_page(store: ProfileStore, cache: ProfileCache, user_id: str) -> Result[Page, StoreError]:
    profile = store.fetch(user_id)?
    except:
        StoreError.Unavailable(_) => cache.last_known(user_id)

    return Ok(render(profile))
```

`ProfileStore`, `ProfileCache`, `Page` and `render` are the application's own. `profile` is the plain value, because of the `?`. On success the block is skipped. On `Unavailable`, the arm's value becomes `profile` and the function continues. `Forbidden` and `Corrupt` have no arm, so they go to the caller, keeping their identity and origin.

An arm can bind the payload it needs, using RFC 137's named patterns:

```incan
size = checked_batch_size(requested, maximum)?
except:
    BatchError.TooLarge(maximum=limit) => limit
```

### Leave from an arm

An arm may leave, as anywhere else. An import loop skips blank rows and lets everything else go to the caller:

```incan
for line in lines:
    row = parse_row(line)?
    except:
        RowError.Blank => continue

    totals.add(row)
```

### Use it without a value

A cleanup step treats "already gone" as success:

```incan
remove_lock(lock_path)?
except:
    LockError.Missing(_) => None
```

### Return a narrower error

Handling a case removes it from what can propagate, so a function can promise less than the call it makes. Under RFC 131, a function must handle excluded cases before returning a narrower set, and this is how:

```incan
error NetworkError:
    Offline
    TimedOut

error StorageError:
    Corrupt(path: str)

error StoreError = NetworkError | StorageError

def cached_profile(store: ProfileStore, cache: ProfileCache, user_id: str) -> Result[Profile, StorageError]:
    profile = store.fetch(user_id)?
    except:
        NetworkError.Offline => cache.last_known(user_id)
        NetworkError.TimedOut => cache.last_known(user_id)

    return Ok(profile)
```

Without those arms, `?` would refuse the line, because `NetworkError` does not fit in `StorageError`. If every case has an arm, nothing is left to propagate, and the function need not return `Result` at all:

```incan
def status_code(request: Request) -> int:
    response = handle(request)?
    except:
        RequestError.NotFound(_) => return 404
        RequestError.Forbidden(_) => return 403
        RequestError.Internal(_) => return 500

    return response.status
```

### When a simpler form fits

A single default is `unwrap_or`, and converting every failure to one application error is `map_err`. `except` earns its place when different errors get different treatment.

### When the recovery fails

A failing recovery propagates its own error, as `or_else` does:

```incan
config = load_config(path)?
except:
    ConfigError.Missing(_) => load_bundled_defaults()?
```

The `?` inside the arm goes straight to the caller, and never re-enters this block. To keep both failures, the author wraps the fallback's error in a case with a field for the original, as RFC 131's constructor form of `map_err` does.

## Reference-level explanation

### Syntax

```text
except_statement ::= statement_ending_in_question_mark NEWLINE
                     "except" ":" NEWLINE INDENT match_arm+ DEDENT
```

1. An `except:` block must follow a binding statement or expression statement whose value expression ends in `?`, aligned with that statement. Binding statements include `name = e?`, `name: T = e?`, `mut name = e?`, reassignment, tuple destructuring and `_ = e?`. Blank lines and comments must not detach the block.
2. `except` must be a contextual keyword, recognized only in this position when followed by `:` and a line break, so that a name `except` and an annotated binding `except: int = 3` keep their meaning.
3. An `except:` after a statement that does not end in `?` must be refused, with a diagnostic pointing at where the `?` belongs. So must an `except:` after a `?` on an `Option`, with a pointer to RFC 134's `let ... else`.

### The block and what it matches

4. The block must apply to the `?` that ends the statement. An inner `?`, as in `checked(parse(raw)?, maximum)?`, must propagate directly and must not reach the block.
5. The source expression must be evaluated exactly once. On `Ok`, the block must be skipped.
6. Arms must be ordinary `match` arms whose subject is the error value of type `E`: ordered dispatch, guards, alternatives, `case` blocks and unreachable-arm checks all apply. A pattern `Ok(...)` or `Err(...)` must be refused, because the subject is the error. A catch-all is `error => ...` or `_ => ...`.
7. Patterns must resolve checked case identities and payload fields under RFC 131 and RFC 137. They must not match rendered messages or search causes. An `Err` carries one error value, and an arm matches that one value, including an aggregate such as RFC 094's `ScopeFailure.Multiple` or an RFC 131 application aggregate, as a whole.

### Arm values and exits

8. An arm whose body is an expression must produce a value assignable to `T`, the payload type. A block arm produces no value and must end by leaving, under the existing match rule.
9. `return`, `break`, `continue` and `?` inside an arm must behave as they do anywhere else in the function, and a failure inside an arm must not re-enter its own block. The matched failure counts as handled, and keeping both failures requires an explicit payload.
10. A name the statement newly binds must not be available inside an arm, and a reassigned `mut` name must still hold its old value there.

### Propagation

11. An error no arm matches must propagate exactly as `?` propagates it, keeping its case, payload and original capture under RFC 131.
12. The remainder must be `E` minus every case handled by an arm with no guard whose pattern matches all of that case's values. The remainder must fit the enclosing function's declared error set under RFC 131's widening rule. If it does not, the program must be refused with the uncovered cases named, as `match` does.
13. If the remainder is empty, the enclosing function need not return `Result`. Otherwise the rules of `?` apply. A foreign `@non_exhaustive` family leaves future cases in the remainder unless an unguarded catch-all arm covers it, under RFC 149.
14. Propagating out of a `with` suite must run its cleanup, under RFC 094. The block must not bypass a resource's required exit or change which failures it retains.

### Diagnostics and tooling

15. Diagnostics must name the source expression, its `T` and `E`, each uncovered case, continuing-arm type mismatches and unreachable arms.
16. Hover must show the source `Result[T, E]`, the bound `T` and the remainder that propagates. Rename, go-to-definition and completion must keep canonical case and field identities. The failure origin must stay distinct from the recovery site.
17. The formatter must keep `except:` aligned with its statement and must not merge it onto one line.

### Conformance workloads

| Workload | Required evidence |
| --- | --- |
| Paths | Success, a recovered error and a propagated error each behave as specified, with exactly-once evaluation and unselected arms unevaluated. |
| Hosts | Every statement form in the syntax accepts a block, and a misplaced `except:` is refused with its diagnostic. |
| Contextual keyword | Existing names `except` and `except: int = 3` keep their meaning. |
| Inner `?` | An inner `?` propagates directly, and a `?` inside an arm does not re-enter the block. |
| Exits | `return`, `break` and `continue` from an arm work, and a block arm that can fall through is refused. |
| Narrowing | A function returns a narrower error set when arms cover the excluded cases, is refused when they do not, and needs no `Result` when nothing remains. |
| Guards | A guarded arm does not remove its case from the remainder. |
| Identity | A propagated error keeps its case, payload and origin. |
| Resources | Propagating out of a `with` suite runs cleanup. |

## Design details

### Why the `?` is written

A form with no `?` was the earlier direction. Its first line read as if the name held a `Result`, it left errors leaving with nothing at the call to show it, and it changed a binding's type based on the next line. Writing the `?` keeps the first line's ordinary meaning and gives one rule for the propagation check. The keyword then reads as English, "`?`, except for these cases", rather than suggesting a thrown exception.

### Why unmatched errors propagate

Without propagation, this form is the `match` expression in the Motivation under another name. Propagation is the feature. It is also what lets a function narrow its error set with no extra machinery.

### Aggregates

RFC 131 decided that an `Err` holds one value, with no implicit error iteration, and RFC 094 selected `ScopeFailure.Multiple` for a body and cleanup that both fail. An arm therefore matches an aggregate as a whole, and handling each member is ordinary iteration in a helper. Python needed a separate `except*` ([PEP 654](https://peps.python.org/pep-0654/)) to dispatch on group members, which this RFC does not add.

### Prior art

Scala's `Try(f()).recover { case e: X => fallback }` takes a partial function and leaves unmatched failures as failures, the closest value-based analog. Swift's `do/catch` propagates uncaught errors from a throwing function and requires exhaustive catches only when the context cannot throw, the same rule as this RFC's empty remainder. Kotlin's `try` is an expression whose unmatched exceptions propagate, and OCaml's `match ... with exception` is similar. Zig's `f() catch |err| switch (err) { ... else => return err }` and Rust's `match` require spelling out propagation, the shape this RFC removes.

## Alternatives considered

- **Keep `match` and combinators only:** workable, but handling one error means writing the `Ok` arm, the `Err` wrappers and the propagation by hand.
- **No `?` on the statement:** see "Why the `?` is written".
- **Exhaustive arms with explicit propagation:** this is `match` under another name.
- **`recover:` or another word:** the mechanism is the same, but `except` reads as a sentence after `?`.
- **Repeated `except X:` clauses:** a single block reuses match arms, bindings and alternatives.
- **An expression form, such as `f(g()? except: ...)`:** would put an indented block inside an expression, which Incan does not allow anywhere.
- **A general `try` block:** intercepting every failure in a suite is a different contract, and hides which call failed.
- **Attaching to `with` only:** ordinary calculations need no resource lifetime.
- **Running several arms for an aggregate's members:** several arms could execute, and an early return could leave members unhandled.
- **Linking the original failure implicitly when recovery fails:** RFC 131's `or_else` requires an explicit payload, and this RFC matches it.

## Drawbacks

Another way to handle errors overlaps `match` and the combinators. A reader who stops at the first line sees errors leaving, and has to read the next line for the exceptions. The word `except` still reminds Python users of exceptions until they see the `?`. An implicit remainder means a newly added error case propagates without a compile error, as with `?`, where an exhaustive `match` would flag it.

## Layers affected

- **Parser and AST:** the contextual keyword and the attached block.
- **Typechecker:** the error-valued subject, arm types, remainder computation and the narrowing check.
- **Lowering and emission:** evaluation, dispatch, exits and propagation with preserved origins.
- **Formatter:** the attached block's layout.
- **Language server:** hovers for `T`, `E` and the remainder, and arm completion.
- **Documentation:** the error-handling guide and reference.

## Design Decisions

- **Form:** a statement ending in `?`, followed by one `except:` block of ordinary match arms, on binding statements and expression statements only. The keyword is contextual.
- **Unmatched errors:** propagate exactly as `?` does, checked against the function's declared error set. Handled cases leave the remainder, so a function can narrow its errors, and an empty remainder needs no `Result`.
- **Arms:** the subject is the error value. An expression arm supplies the replacement, a block arm must leave, and exits behave as they do anywhere else.
- **Scope:** the block covers only the `?` ending its line. An inner `?` and a `?` inside an arm propagate directly, with no re-entry.
- **Aggregates:** an arm matches one error value as a whole, under RFC 131 and RFC 094.
- **Failed recovery:** propagates its own error, as `or_else` does. Keeping both requires an explicit payload.
