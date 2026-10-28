# Codegraph throughput

Measured October 28, 2026 on macOS arm64 against parent `48b16d5735d51f20fb4820d5a70bd3b348516663`, using the unmodified `compile_scaling` harness at `48f2f0528c97fe2f4ae3a02d0f5d3d0a165bd51d`. Release debug information remains level 2, with two native build tokens. These are single observations admitted with load at most 3, memory free at least 50% and disk free at least 60 GB.

| Command | Parent seconds | Current seconds |
| --- | ---: | ---: |
| Lowering cold | 291.278 | 253.789 |
| Lowering unchanged | 1.780 | 1.848 |
| Lowering body edit | 261.035 | 211.216 |
| std.data cold | 29.884 | 29.265 |
| std.data unchanged | 26.866 | Not admitted |
| std.data body edit | 27.137 | Not admitted |

The coordinator's wrap-up cutoff ended the harness while it awaited admission for the last two trials; load was 4.83. All four completed commands succeeded. The lowering edit was reversed and its source SHA-256 verified; std.data was never edited. Cold withdraws the subject's retained build proof. Command times include orchestration and, for lowering, its compiler consumer's relink.

The lowering invocation itself took 205.860 seconds cold and 195.061 after the body edit, compared with 243.498 and 244.581 in the parent baseline. The edited invocation decreased by 20.2%; its command decreased by 19.1%. The parent's 42.791-second cold graph collection span is absent. Peak physical footprint across the two final lowering invocations was 4,879,456,904 bytes, within 6 GiB; std.data's cold invocation peaked at 1,823,853,688 bytes, within 2 GiB. The kernel lifetime maximum supplies these readings.

Correctness: graph 4/4 with the live LSP contract; census 1,214 PASS, zero FAIL, one inherited PEND with exact parent verdict parity; Oven 69/69 with exact test IDs; 18 Rust facets; 23 runtime/standard source builds; quality and strict docs build passed. Existing graph cases additionally compare complete retained and fresh documents, changed producer generations, requested publication reuse and malformed publication refusal.

Incan native compilation performs no graph collection. A first requested publication may require a separate checking pass; unchanged requests retain the checked export. Reuse is by complete unit inputs. Incremental module fragments are not implemented, and Rust graph production keeps its existing policy.
