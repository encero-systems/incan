# `implicit_borrowing_clone_counter`

A project fixture whose Rust crate, `rust/borrow_fixture`, counts copies. `Clone for Item` adds one to a counter for every node it copies, and `clones()` reads the counter. The clone of a `Table` clones its map, so a copy costs one count per node copied: `document()` (`Table{project: Table{count: Integer(7)}}`) costs 3, its `project` child costs 2, and an `Integer` leaf costs 1.

Each expected line of the program (`main.incn`) is a program step, the step's result, and the running count after it. A step that lends its value adds nothing; a step that copies adds the size of what it copies. Checked ownership facts reach native lowering through `loaves/compiler/incan_frontend/src/body_ir/reads.rs`; the contract measures the resulting copies rather than depending on the retired emitter's borrow-inference passes.

| Line | Program step | Copies | Why |
|---|---|---|---|
| `observe` | `observe` -> `traverse`, a cursor over `get` children | 0 | `get(&self) -> Option<&Item>` is a shared receiver borrow; `item` and `current` are lent |
| `is_table` | `Holder.create` stores a tree, then `Holder.observe` -> private `is_table(self.item)` reads it | 0 | `create` consumes its parameter on entry, so it keeps a by-value input and the fresh tree moves into the model field; the later reads lend it |
| `inspect` | `inspect` -> `classify` -> `is_table` | 0 | the private helpers take `&Item` and the public entry lends its parameter |
| `shapes` | format, enum match, index, indexed text and byte field reads, helper/comparison/method reads, nested iteration, string comparison/membership and unrelated body forms | 0 | each operation observes its input; the byte case mirrors Crumb's field identity read and a read after an indexed field write sees current element storage, while unrelated aggregates, results and closures do not change that ownership proof |
| `indexed items` | `for item in envelopes[0].items`, where the nested list contains one three-node tree | +3 = 3 | the indexed nested list is lent without a preliminary copy; the loop variable is an independent snapshot and therefore copies its one tree exactly once |
| `ref local` | `Holder.view`: `current: &Item = self.item` | 0 | a reference-typed local is a borrow, never a materialization |
| `walk` | `Holder.walk`: cursor started from `self.item` | 0 | the cursor is proven shared and the owner is not read again in the body |
| `selected` | `Reader.read` -> `select`, ending in `current.clone()` | +1 = 4 | the explicit clone copies the `Integer(7)` leaf; the walk to it lends |
| `owned child` | `take_child`: an owned cursor calls `child()` and then stores that result | +2 = 6 | `take_child` consumes its parameter on entry into the owned cursor, so the fresh tree moves in; `child()` copies the two-node `project` subtree |
| `snapshot` | a second `Holder.create`, then `Holder.snapshot`: `current = self.item` before the field is replaced | +3 = 9 | `create` moves the fresh tree into the field; the later receiver snapshot copies those three nodes before replacement |
| `ref param` | `is_table_ref(item: &Item)` directly and through `check_ref` | 0 | a declared reference parameter borrows its argument, in a return too |
| `ref traverse` | `traverse_ref(item: &Item, ...)` reassigning its cursor | 0 | `current = child` keeps the borrowed shape |
| `callback` | `callback()` returns `is_table_callback`, called on a fresh tree | 0 | the function-item adapter owns the temporary and lends it to the classifier's shared input |
| `pair` | `observe_pair` -> `classify_pair(item, item)` | 0 | both immutable inputs borrow the same caller storage, including the read that is not the last; neither read needs an independent value |
| `select` | `select_owned`: the cursor is returned | 0 | `select_owned` consumes its parameter on entry into the owned cursor, so the fresh tree moves in and out as the result |
| `clear` | `clear`: an owned local calls `current.clear()`, then `child_count()` is 0 | 0 | the mutable local consumes the parameter on entry, so the fresh tree moves in and is cleared without a copy |
| `trait callback` | `Factory.callback` default returns `is_table_callback`, called once | 0 | the same escaping classifier, reached through a trait default |
| `exclusive forward` | `forward_clear_ref` -> `clear_ref`, then `child_count()` is 0 | 0 | both helpers declare `mut item: &mut Item`, so the exclusive loan forwards the original owner |
| `skipped store` | a conditional storing helper called on its non-storing path with a value the caller keeps | +3 = 12 | the helper consumes its parameter on one path, so it keeps its by-value input as on the base, and the caller copies the three-node tree it keeps |
| `stored copy` | the same helper stores its parameter, then the caller clears the original | +3 = 15 | the caller copies the three-node tree it keeps and the storing path moves that copy into the saved model |
| `fresh store` | the helper called on its storing path with a fresh tree | 0 | the fresh tree moves into the by-value input and on into the saved model |
| `fresh skip` | the helper called on its non-storing path with a fresh tree | 0 | the fresh tree moves into the by-value input and is dropped there |

`is_table` and `is_table_callback` are two functions on purpose: a classifier that escapes as a callable value uses an owned function-item adapter whose internal call lends the argument, while one that only a direct call reaches needs no adapter. One function cannot show both.

The first three counts (`observe` 0, `Reader.read` 1, `Holder.snapshot` +3) are the ones the retired `inferred_traversal_compiles_and_runs_without_any_tree_clones` consumer asserts.
