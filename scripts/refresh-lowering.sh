#!/bin/zsh
# Reuse a complete stage-zero lowering emission, or refresh only generated files whose bytes changed.
set -e
set -o pipefail
zmodload zsh/datetime

lane=${DEV8_LANE:?set DEV8_LANE to the bootstrap lane directory}
bootstrap=$lane/bootstrap
repo=$lane/incan
project=$bootstrap/ambient2/incan_mir_lowering
destination=$bootstrap/loaves/incan_mir_lowering/src
stamp=$bootstrap/lowering-emission.sha256
script=${0:A}

# Stage zero and its source/dependency snapshot are frozen bootstrap inputs, not the installed output driver.
input_digest() {
    local files=("$script" "$bootstrap/scripts/stage0-boot.sh" /Volumes/Builds/dev8-fix/bootstrap/bootbin/incan
        "$project/loaf.toml" "$project/oven.lock" "$repo/loaves/compiler/incan_mir_plan/src/lib.incn")
    files+=("$repo"/loaves/compiler/incan_mir_lowering/src/**/*.incn(N.))
    files+=("$bootstrap"/kernel/**/*(N.))
    shasum -a 256 "${files[@]}" | shasum -a 256 | cut -d ' ' -f1
}

output_digest() {
    local files=("$destination"/**/*.rs(N.))
    (( ${#files} > 0 )) || return 1
    shasum -a 256 "${files[@]}" | shasum -a 256 | cut -d ' ' -f1
}

inputs=$(input_digest)
if [[ -f $stamp ]] && [[ $(head -1 "$stamp") == $inputs ]]; then
    outputs=$(output_digest) || outputs=
    if [[ -n $outputs && $(tail -1 "$stamp") == $outputs ]]; then
        print 'lowering emission reused'
        exit 0
    fi
fi

# The frozen emitter checks and emits the whole module graph. Its output is scratch until emission succeeds.
rm -rf "$project/src" "$project/target/lib"
cp -R "$repo/loaves/compiler/incan_mir_lowering/src" "$project/src"
cp "$repo/loaves/compiler/incan_mir_plan/src/lib.incn" "$project/src/incan_mir_plan.incn"
sed -i '' 's/pub::incan_mir_plan/incan_mir_plan/g' "$project"/src/*.incn
sed -i '' -E 's/[[:<:]]Operand[[:>:]]/RustOperand/g; s/PlaceElem, RustOperand, Constant/PlaceElem, Operand as RustOperand, Constant/; s/RustOperand as PlanOperand/Operand/; s/[[:<:]]PlanOperand[[:>:]]/Operand/g' "$project/src/tuples.incn"
sed -i '' -E 's/[[:<:]]EnumDeclaration[[:>:]]/RustEnumDeclaration/g; s/^    RustEnumDeclaration,$/    EnumDeclaration as RustEnumDeclaration,/; s/RustEnumDeclaration as NativeEnumDeclaration/EnumDeclaration/; s/[[:<:]]NativeEnumDeclaration[[:>:]]/EnumDeclaration/g' "$project/src/value_enums.incn"
sed -i '' -E 's/[[:<:]]OperandPlace[[:>:]]/Place/g; s/Place as Place/Place/' "$project/src/model_lists.incn"
started=$EPOCHREALTIME
(cd "$project" && "$bootstrap/scripts/stage0-boot.sh" build --lib > "$bootstrap/boot-lib.log" 2>&1) || true
print -r -- "stage0 emission seconds: $((EPOCHREALTIME-started))"
errors=$(sed 's/\x1b\[[0-9;]*m//g' "$bootstrap/boot-lib.log" | grep -c '^type error\|^syntax error' || true)
if [[ $errors != 0 || ! -f $project/target/lib/src/lib.rs ]]; then
    print -u2 "emission failed ($errors errors); see $bootstrap/boot-lib.log"
    exit 1
fi

staging=$(mktemp -d "$bootstrap/lowering-emission.XXXXXX")
trap 'rm -rf "$staging"' EXIT
cp -R "$project/target/lib/src/" "$staging/"
cat >> "$staging/lib.rs" <<'EOF'

/// Bootstrap: the caller projection a dev.7 project bake would emit for Rust consumers.
pub mod caller {
    pub mod incan {
        pub use crate::*;
    }
}
EOF
sed -i '' 's/PartialEq::eq(declaration.canonical, owner)/PartialEq::eq(\&declaration.canonical, owner)/' "$staging/traits.rs"
if [[ $(input_digest) != $inputs ]]; then
    print -u2 'lowering inputs changed during emission; retry the refresh'
    exit 1
fi
mkdir -p "$destination"
# Compare before copying: unchanged files retain their inode and all timestamps, not just their content.
for generated in "$staging"/**/*.rs(N.); do
    relative=${generated#$staging/}
    installed=$destination/$relative
    if [[ -f $installed ]] && cmp -s "$generated" "$installed"; then
        continue
    fi
    mkdir -p "${installed:h}"
    cp "$generated" "$installed"
    print -r -- "lowering changed: $relative"
done
for installed in "$destination"/**/*.rs(N.); do
    relative=${installed#$destination/}
    if [[ ! -f $staging/$relative ]]; then
        rm "$installed"
        print -r -- "lowering removed: $relative"
    fi
done
outputs=$(output_digest)
print -rl -- "$inputs" "$outputs" > "$stamp.new"
mv "$stamp.new" "$stamp"
print -r -- "lowering refreshed: $(find "$destination" -name '*.rs' | wc -l | tr -d ' ') Rust files"
