# Work with a multi-project workspace

Use a workspace when several ordinary Incan projects share one repository and should use deterministic command selection, reusable dependency declarations, and one canonical `incan.lock`. Each member keeps its own project name, version, source tree, and publication lifecycle.

## Create two member projects

Start with a virtual workspace root and two ordinary projects:

```bash
mkdir -p service-workspace/packages
cd service-workspace
incan new --dir packages/api --yes
incan new --dir packages/worker --yes
```

Create `incan.toml` at `service-workspace/incan.toml`:

```toml
[workspace]
members = ["packages/*"]
default-members = ["api"]
```

The root is virtual because it has `[workspace]` but no `[project]`. A virtual root must select at least one member. Every member selected by `members` must contain its own `incan.toml` with a unique project name.

Run the inspector before building:

```bash
incan workspace inspect
incan workspace inspect --format json
```

The human view is useful while editing the workspace. The JSON view is the machine-readable contract for member roots, selection origin, inherited dependencies and environments, lock state, unused shared declarations, and stale member-local locks.

## Choose command scope explicitly

Commands started at the virtual root select `default-members` when that list is present:

```bash
# Checks only api because api is the default member.
incan check

# Builds every member in deterministic workspace order.
incan build --workspace --report json

# Tests one member selected by project name or root-relative path.
incan test --member worker
incan test --member packages/worker
```

When a command starts inside `packages/api`, the current member is selected automatically:

```bash
cd packages/api
incan test
```

`check`, `build`, `test`, and `fmt` may fan out across several selected members. `run` and `version` require exactly one member, so use `--member` when the root selection would contain more than one project.

## Share dependency identity without enabling it everywhere

Declare a reusable Rust dependency at the workspace root:

```toml
[workspace]
members = ["packages/*"]
default-members = ["api"]

[workspace.rust-dependencies]
serde = { version = "1", default-features = false }
```

Then opt a member into that declaration from `packages/api/incan.toml`:

```toml
[project]
name = "api"
version = "0.1.0"

[rust-dependencies]
serde = { workspace = true, features = ["derive"] }
```

The root owns the dependency's version, source, package rename, and default-feature baseline. A member may add features and choose whether its own use is optional. Merely declaring a shared dependency at the root does not enable it in every member.

## Generate and commit the root lock

Run locking from the root or any member:

```bash
incan lock
incan workspace inspect --format json
```

The workspace always publishes one canonical `incan.lock` at the workspace root. Do not commit member-local lockfiles as authorities; the inspector reports any stale member-local locks so they can be removed deliberately. Builds and tests attribute their semantic and backend closure to the selected member while consuming the same root lock.

## Choose rooted or virtual layout deliberately

Add `[project]` to the root only when the root is itself an application or library:

```toml
[project]
name = "service_tools"
version = "0.1.0"

[workspace]
members = ["packages/*"]
default-members = ["."]
```

This is a rooted workspace. The root project becomes a member automatically, so do not add `"."` to `members`. `"."` is valid in `default-members` and `--member .` when you want to select the root project.

## Diagnose topology failures

Run `incan workspace inspect` after changing membership. Common failures have specific remedies:

| Failure                                                                       | Fix                                                                                                            |
| ----------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| A literal member path has no `incan.toml`.                                    | Correct the root-relative path or initialize the member project.                                               |
| A member, exclusion, or selector escapes the workspace root.                  | Use a non-empty root-relative path that remains beneath the canonical root.                                    |
| Two members use the same project name.                                        | Give every member a unique `[project].name`; name-based selection must be unambiguous.                         |
| A `default-members` entry matches no member or is ambiguous.                  | Use a unique project name or root-relative member path.                                                        |
| A rooted workspace lists `"."` in `members`.                                  | Remove it; `[project]` already establishes root membership.                                                    |
| A glob matches no project.                                                    | Treat the inspector warning as drift: create the intended member or correct the pattern.                       |
| A member requests `{ workspace = true }` for an undeclared shared dependency. | Add the declaration under the matching `[workspace.*-dependencies]` table or make the dependency member-local. |

Use exclusions for intentionally omitted paths such as `packages/experimental`, not to hide malformed members. An exclusion is applied after member expansion and may name a temporarily absent path, which keeps repository transitions inspectable.

## See also

- [Project lifecycle reference](../reference/project_lifecycle.md#workspaces)
- [Project configuration](../../tooling/reference/project_configuration.md)
- [SDK components and package features](../../tooling/how-to/sdk_components_and_package_features.md)
- [CLI reference](../../tooling/reference/cli_reference.md)
