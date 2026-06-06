#!/bin/bash

# Incan Benchmark Suite
# Compares Incan, Rust, and Python performance

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
RESULTS_DIR="$SCRIPT_DIR/results"
RESULTS_FILE="$RESULTS_DIR/results.md"

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${BLUE}=== Incan Benchmark Suite ===${NC}"
echo ""

# Check for hyperfine
if ! command -v hyperfine &> /dev/null; then
    echo -e "${YELLOW}Warning: hyperfine not found. Install with: brew install hyperfine${NC}"
    echo "Falling back to basic timing..."
    USE_HYPERFINE=false
else
    USE_HYPERFINE=true
fi

# Build Incan compiler in release mode
echo -e "${GREEN}Building Incan compiler...${NC}"
cd "$PROJECT_ROOT"
cargo build --release --quiet
INCAN="$PROJECT_ROOT/target/release/incan"

# Initialize results file
mkdir -p "$RESULTS_DIR"
echo "# Benchmark Results" > "$RESULTS_FILE"
echo "" >> "$RESULTS_FILE"
echo "Generated: $(date)" >> "$RESULTS_FILE"
echo "" >> "$RESULTS_FILE"
echo "| Benchmark | Incan | Rust | Python | Incan vs Python |" >> "$RESULTS_FILE"
echo "|-----------|-------|------|--------|-----------------|" >> "$RESULTS_FILE"

# Function to run a single benchmark
run_benchmark() {
    local name=$1
    local dir=$2
    local basename=$3
    local skip_rust="${SKIP_RUST:-false}"
    local skip_python="${SKIP_PYTHON:-false}"
    local incan_only="${INCAN_ONLY:-false}"
    local build_only="${BUILD_ONLY:-false}"
    
    echo -e "${GREEN}Running: $name${NC}"
    cd "$dir"
    
    if [ "$incan_only" = true ]; then
        skip_rust=true
        skip_python=true
    fi

    # Build Incan version (use absolute path so target goes to project root)
    echo "  Building Incan..."
    cd "$PROJECT_ROOT"
    if ! $INCAN build "$dir/$basename.incn" 2>"/tmp/incan_build_${basename}.log"; then
        echo "  Failed to build Incan version"
        echo "  ---- compiler output ----"
        sed -n '1,200p' "/tmp/incan_build_${basename}.log" || true
        echo "  -------------------------"
        return 1
    fi
    cd "$dir"
    # Generated crates share `target/incan/.cargo-target/release/` (see `ProjectGenerator::cargo_target_dir`).
    local incan_bin="$PROJECT_ROOT/target/incan/.cargo-target/release/$basename"
    cp "$incan_bin" "${basename}_incan" || {
        echo "  Failed to copy Incan binary"
        return 1
    }
    
    # Build Rust version
    if [ "$skip_rust" != true ]; then
        echo "  Building Rust..."
        rustc -O "$basename.rs" -o "${basename}_rust" 2>/dev/null || {
            echo "  Failed to build Rust version"
            return 1
        }
    fi

    if [ "$build_only" = true ]; then
        rm -f "${basename}_incan" "${basename}_rust" 2>/dev/null || true
        echo ""
        return 0
    fi
    
    if [ "$USE_HYPERFINE" = true ]; then
        # Run with hyperfine
        echo "  Benchmarking..."
        if [ "$skip_python" = true ] && [ "$skip_rust" = true ]; then
            hyperfine --warmup 2 --min-runs 5 \
                "./${basename}_incan" \
                --export-json "/tmp/bench_${basename}.json"
        elif [ "$skip_python" = true ]; then
            hyperfine --warmup 2 --min-runs 5 \
                "./${basename}_incan" \
                "./${basename}_rust" \
                --export-json "/tmp/bench_${basename}.json"
        elif [ "$skip_rust" = true ]; then
            hyperfine --warmup 2 --min-runs 5 \
                "./${basename}_incan" \
                "python3 $basename.py" \
                --export-json "/tmp/bench_${basename}.json"
        else
            hyperfine --warmup 2 --min-runs 5 \
                "./${basename}_incan" \
                "./${basename}_rust" \
                "python3 $basename.py" \
                --export-json "/tmp/bench_${basename}.json"
        fi
        
        # Extract times
        INCAN_TIME=$(jq -r '.results[0].mean * 1000 | floor' "/tmp/bench_${basename}.json")
        RUST_TIME="-"
        PYTHON_TIME="-"
        if [ "$skip_python" != true ] && [ "$skip_rust" != true ]; then
            RUST_TIME=$(jq -r '.results[1].mean * 1000 | floor' "/tmp/bench_${basename}.json")
            PYTHON_TIME=$(jq -r '.results[2].mean * 1000 | floor' "/tmp/bench_${basename}.json")
        elif [ "$skip_python" = true ] && [ "$skip_rust" != true ]; then
            RUST_TIME=$(jq -r '.results[1].mean * 1000 | floor' "/tmp/bench_${basename}.json")
        elif [ "$skip_rust" = true ] && [ "$skip_python" != true ]; then
            PYTHON_TIME=$(jq -r '.results[1].mean * 1000 | floor' "/tmp/bench_${basename}.json")
        fi
        
        # Calculate speedup
        if [ "$PYTHON_TIME" != "-" ] && [ "$PYTHON_TIME" -gt 0 ] && [ "$INCAN_TIME" -gt 0 ]; then
            SPEEDUP=$(echo "scale=1; $PYTHON_TIME / $INCAN_TIME" | bc)
            SPEEDUP_STR="${SPEEDUP}x faster"
        else
            SPEEDUP_STR="N/A"
        fi
        
        echo "| $name | ${INCAN_TIME}ms | ${RUST_TIME}ms | ${PYTHON_TIME}ms | $SPEEDUP_STR |" >> "$RESULTS_FILE"
    else
        # Fallback: simple timing
        echo "  Running Incan..."
        INCAN_TIME=$( { time ./"${basename}_incan" > /dev/null; } 2>&1 | grep real | awk '{print $2}' )
        
        RUST_TIME="-"
        if [ "$skip_rust" != true ]; then
            echo "  Running Rust..."
            RUST_TIME=$( { time ./"${basename}_rust" > /dev/null; } 2>&1 | grep real | awk '{print $2}' )
        fi
        
        PYTHON_TIME="-"
        if [ "$skip_python" != true ]; then
            echo "  Running Python..."
            PYTHON_TIME=$( { time python3 "$basename.py" > /dev/null; } 2>&1 | grep real | awk '{print $2}' )
        fi
        
        echo "| $name | $INCAN_TIME | $RUST_TIME | $PYTHON_TIME | - |" >> "$RESULTS_FILE"
    fi
    
    # Cleanup binaries
    rm -f "${basename}_incan" "${basename}_rust"
    
    echo ""
}

# Run all benchmarks
echo ""
echo -e "${BLUE}=== Compute Benchmarks ===${NC}"
run_benchmark "Fibonacci (1M)" "$SCRIPT_DIR/compute/fib" "fib"
run_benchmark "Collatz (1M)" "$SCRIPT_DIR/compute/collatz" "collatz"
run_benchmark "GCD (10M)" "$SCRIPT_DIR/compute/gcd" "gcd"
run_benchmark "Mandelbrot (2K)" "$SCRIPT_DIR/compute/mandelbrot" "mandelbrot"
run_benchmark "N-Body (500K)" "$SCRIPT_DIR/compute/nbody" "nbody"
run_benchmark "Prime Sieve (50M)" "$SCRIPT_DIR/compute/primes" "primes"

echo ""
echo -e "${BLUE}=== Sorting Benchmarks ===${NC}"
run_benchmark "Quicksort (1M)" "$SCRIPT_DIR/sorting/quicksort" "quicksort"
run_benchmark "Mergesort (1M)" "$SCRIPT_DIR/sorting/mergesort" "mergesort"

echo ""
echo -e "${GREEN}=== Results ===${NC}"
cat "$RESULTS_FILE"
echo ""
echo -e "${GREEN}Results saved to: $RESULTS_FILE${NC}"
