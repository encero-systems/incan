# Build and verify a release locally

Use this procedure to produce the toolchain archive for your host from one commit and verify it as an installed toolchain, on your own machine. It is the same program CI runs, so a local run answers the same question without a push.

## Before you start

- **An installed seed toolchain.** The release is built by an installed toolchain, never by the checkout. Select it with `INCAN_HOME`; it must contain `bin/incan` and `bin/incan-lsp`. Set `INCAN_LSP` when the language server lives elsewhere. [Install and run](../../tooling/how-to/install_and_run.md) shows how to install a toolchain archive.
- **Rust 1.98.0 with `rustc-dev`.** Installing the seed already requires it.
- **A commit.** The release clones the exact commit you name and builds that. Uncommitted changes are not part of it.
- **A new output directory.** The release refuses a directory that already exists.
- **A clean environment.** Leave `INCAN_STDLIB` and `INCAN_SOURCE_ROOT` unset. They select checkout sources, which is the opposite of what this procedure verifies.

## Run the release

From the repository root:

```sh
incan run scripts/release.incn -- "$(git rev-parse HEAD)" /path/to/new/output
```

The commit must be written as its full lowercase SHA, which `git rev-parse` prints.

Options:

- `--jobs N` and `--timeout SECONDS` set the process budgets: how many cases run at once, and how long one case may take. The defaults are 4 and 600.
- `--prove-fixed-point` rebuilds the toolchain with itself twice more and requires the second and third generations to be byte-identical. Release production uses it; an ordinary check can leave it out, because it builds two more generations.
- `--keep-trees` keeps the work trees that a passing run otherwise removes after measuring them.

The program exits `0` and prints `release receipt:` with the receipt's path when the release qualifies. It exits `1` when a phase refuses the release, and `2` when the arguments are wrong.

## What the release does

1. Clones the commit and prepares a fresh home bound to the seed.
2. Builds the compiler, runtime, standard Loaves, Oven and the language server from that source.
3. Packs the archive.
4. Runs the contract census with the seed, which defines the cases and the outcomes that must be kept.
5. Installs the archive into a second home and verifies that installation: relocation, Oven tests, the Rust facets, driver integration, the language server, the contract census and memory.

## What you get

Everything is written below the output directory:

| Path | Content |
| --- | --- |
| `bundle/incan`, `bundle/incan-lsp`, `bundle/toolchain.tar.zst` | The release for your host |
| `receipt.json` | The outcome, counts and digests; a failed phase leaves a refused receipt |
| `timings.tsv`, `memory.tsv` | Time and memory per phase |
| Per-phase logs | The complete output of each phase |
| `ci/run.*/` | Evidence from verifying the installed archive |

To try the result by hand, copy `bundle/incan` and `bundle/incan-lsp` into the `bin` directory of a new home, select that home with `INCAN_HOME`, and run `incan toolchain install` on `bundle/toolchain.tar.zst`.

## When a checkout is not the place to verify

A command run from a development setup reads standard sources from the checkout. That proves the sources. It does not prove what an archive installs.

Some defects exist only in the installed form: a file the archive does not ship, a path that stops resolving once the toolchain is moved, an artifact built for a different profile. No run against checkout sources can see them. Verify against an installed archive when a change touches:

- what the archive contains or how it is packed and installed;
- how the standard Loaves or their compiled artifacts are found;
- paths recorded in anything the toolchain reads back later.

To verify a toolchain that is already installed, without building one, run the verification step alone:

```sh
INCAN_HOME=/path/to/installed/home incan run scripts/verify_installation.incn -- /path/to/new/evidence
```

It refuses an installation that ships no standard sources, so it cannot fall back to the checkout's.

## See also

- [Toolchain release design](../explanation/toolchain_release_design.md) explains why releases are built this way.
- `scripts/README.md` in the repository lists every release and verification program.
