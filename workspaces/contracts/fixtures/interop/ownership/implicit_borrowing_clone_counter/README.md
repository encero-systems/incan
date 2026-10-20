# `implicit_borrowing_clone_counter`

A project fixture whose Rust crate, `rust/borrow_fixture`, counts copies. `Clone for Item` adds one to a counter for every node it copies, and `clones()` reads the counter. The clone of a `Table` clones its map, so a copy costs one count per node copied: `document()` (`Table{project: Table{count: Integer(7)}}`) costs 3, its `project` child costs 2, and an `Integer` leaf costs 1.

Each expected line of the program (`main.incn`) is a program step, the step's result, and the running count after it. A step that lends its value adds nothing; a step that copies adds the size of what it copies. The borrow inference in `loaves/compiler/incan_ir/src/borrow_inference.rs` decides which: its `SharedUse` proof accepts only shared receiver contracts and same-root cursor reassignments, and its `Escapes` pass keeps the owned ABI for a function named outside a direct call or called with overlapping arguments.

| Line | Program step | Copies | Why |
|---|---|---|---|
| `observe` | `observe` -> `traverse`, a cursor over `get` children | 0 | `get(&self) -> Option<&Item>` is a shared receiver borrow; `item` and `current` are lent |
| `is_table` | `Holder.observe` -> private `is_table(self.item)` | 0 | `is_table` is only ever a direct callee, so it takes `&Item`; the field is lent |
| `inspect` | `inspect` -> `classify` -> `is_table` | 0 | the private helpers take `&Item` and the public entry lends its parameter |
| `ref local` | `Holder.view`: `current: &Item = self.item` | 0 | a reference-typed local is a borrow, never a materialization |
| `walk` | `Holder.walk`: cursor started from `self.item` | 0 | the cursor is proven shared and the owner is not read again in the body |
| `selected` | `Reader.read` -> `select`, ending in `current.clone()` | +1 = 1 | the explicit clone copies the `Integer(7)` leaf; the walk to it lends |
| `owned child` | `take_child`: `current = child` from `child()` | +2 = 3 | `child()` returns an owned `Item` (`get(key).cloned()`), copying the `project` subtree; the cursor stays owned |
| `snapshot` | `Holder.snapshot`: `current = self.item`, then the field is replaced | +3 = 6 | the owner is written after the read, so the read materializes a copy of the whole tree |
| `ref param` | `is_table_ref(item: &Item)` directly and through `check_ref` | 0 | a declared reference parameter borrows its argument, in a return too |
| `ref traverse` | `traverse_ref(item: &Item, ...)` reassigning its cursor | 0 | `current = child` keeps the borrowed shape |
| `callback` | `callback()` returns `is_table_callback`, called on a fresh tree | 0 | the escaping classifier keeps the owned ABI, so the temporary is moved in |
| `pair` | `observe_pair` -> `classify_pair(item, item)` | +3 = 9 | overlapping arguments keep `classify_pair` owned; the first `item` is not its last use and is copied whole, the second is moved |
| `select` | `select_owned`: the cursor is returned | 0 | a returned cursor is an escape, so `item` and `current` stay owned and move |
| `clear` | `clear`: `current.clear()`, then `child_count()` is 0 | 0 | `clear(&mut self)` is not a shared contract, so the parameter stays owned and moves |
| `trait callback` | `Factory.callback` default returns `is_table_callback`, called once | 0 | the same escaping classifier, reached through a trait default |

`is_table` and `is_table_callback` are two functions on purpose: a classifier that escapes as a callable value keeps the owned ABI, and one that only a direct call reaches is lent its argument. One function cannot show both.

The first three counts (`observe` 0, `Reader.read` 1, `Holder.snapshot` +3) are the ones the retired `inferred_traversal_compiles_and_runs_without_any_tree_clones` consumer asserts.
