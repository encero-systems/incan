To build the 0.5 toolchain from source, use the matching release checkout and prepare its bounded release Loaf envelope:

```bash
git clone https://github.com/encero-systems/incan.git
cd incan
git switch --detach v0.5.0
export INCAN_CHECKOUT="$PWD"
make build
make test-prewarm-oven-release-loafs \
  INCAN_TEST_COMPILER_ALREADY_BUILT=1 \
  INCAN_TEST_OVEN_RELEASE_COMPILER_BIN="$PWD/target/debug/incan"
export INCAN_SOURCE_ROOT="$INCAN_CHECKOUT"
export INCAN_STDLIB="$INCAN_CHECKOUT/crates/incan_stdlib/stdlib"
export INCAN_STDLIB_DIR="$INCAN_STDLIB"
export INCAN_TOOLCHAIN_CRATES_DIR="$INCAN_CHECKOUT/crates"
export PATH="$INCAN_CHECKOUT/target/oven-alpha-release-toolchain/bin:$PATH"
incan --version
```

`make build` compiles the compiler and language server. The explicit prewarm target then assembles the complete 0.5 release envelope used by repository smoke tests—including the debug and release full-standard-library Loaf families—and puts its compiler under `target/oven-alpha-release-toolchain/bin`. The checkout variables keep examples tied to the same compiler and standard library after you change directories; they name the layout of the `v0.5.0` checkout the recipe pins, which is why the recipe and the tag go together. This is a source-checkout preparation step, not a normal project build command. Keep that prepared binary first on `PATH` while using the source-built toolchain, and repeat both commands after updating the checkout.
