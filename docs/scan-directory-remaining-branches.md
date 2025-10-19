## Remaining Scan-Directory Coverage Gaps (2025-10-19 15:36)

- Despite the alias fallback test, lines ~3337-3374 remain uncovered—build a single traversal that yields two sibling entries resolving to the same canonical directory without reinserting stats post-scan.
- Alternating coverage now includes ignore/max-depth permutations, yet lines around ~3680 still show misses—explore deeper mixes (e.g., ignore branches that also trigger max-depth warnings mid-traversal) to exercise every fallback lookup.
