#!/usr/bin/env bash
# Golden set runner for lexical semantic tier (graphzero-nmf).
# Reports top-1 and top-3 file accuracy INCLUDING losses.
set -euo pipefail

GZ_BIN="${GZ_BIN:-./target/release/graphzero}"
GZ_REPO="${GZ_REPO:-.}"
# No host-specific default: the pi-stack corpus lives outside this repo, and a
# default pointing at one machine's home directory silently measured nothing
# everywhere else.
PI_REPO="${PI_REPO:-$HOME/Developer/pi-stack}"

# Golden queries: "query|repo|expected_file_substring"
# repo = "gz" for graphzero, "pi" for pi-stack
GOLDEN_QUERIES=(
  "snap routing and query capsule|gz|store/query/snap.rs"
  "BM25 scoring over inverted index|gz|store/query/lexical.rs"
  "CSR adjacency edge iteration|gz|store/csr.rs"
  "symbol table perfect hash lookup|gz|store/symbol_table.rs"
  "blob store content addressable storage|gz|store/blob_store.rs"
  "indexer walk and extract definitions|gz|store/indexer.rs"
  "coverage bitmap tier counting|gz|store/coverage.rs"
  "delta log segment replay|gz|store/delta_log.rs"
  "name bigram candidate filtering|gz|store/query/name_bigram.rs"
  "manifest snapshot entry loading|gz|store/manifest.rs"
  "expand gz ref to bytes|gz|store/expand.rs"
  "freshness staleness diagnostic check|gz|store/query/freshness.rs"
  "pi ask user interface|pi|pi-ask-user/index.ts"
  "pi atp operations config|pi|pi-atp/operations.js"
  "pi deferred context engine|pi|pi-deferred-context-engine"
  "pi powerline footer display|pi|pi-powerline-footer"
  "pi style formatting|pi|pi-style"
  "pi subagents orchestration|pi|pi-subagents"
  "pi tidy tools cleanup|pi|pi-tidy-tools"
  "pi web access module|pi|pi-web-access"
)

pass_top1=0
pass_top3=0
total=0

printf "%-55s | %-8s | %-8s | %s\n" "Query" "Top-1" "Top-3" "Expected"
printf "%-55s-+-%-8s-+-%-8s-+-%s\n" "---------------------------------------" "--------" "--------" "------------------------"

for entry in "${GOLDEN_QUERIES[@]}"; do
  query="${entry%%|*}"
  rest="${entry#*|}"
  repo_code="${rest%%|*}"
  expected="${rest#*|}"

  if [[ "$repo_code" == "gz" ]]; then
    repo="$GZ_REPO"
  else
    repo="$PI_REPO"
  fi

  result=$("$GZ_BIN" snap "$query" --budget 4096 --repo "$repo" 2>/dev/null || echo '{}')

  paths=$(echo "$result" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    dests = d.get('destinations', [])
    paths = [dd.get('path','') for dd in dests]
    print('\n'.join(paths))
except:
    pass
" 2>/dev/null)

  top1_path=$(echo "$paths" | head -1)
  top3_paths=$(echo "$paths" | head -3)

  total=$((total + 1))
  hit_top1=0
  hit_top3=0

  if echo "$top1_path" | grep -qi "$expected" 2>/dev/null; then
    hit_top1=1
    pass_top1=$((pass_top1 + 1))
  fi

  if echo "$top3_paths" | grep -qi "$expected" 2>/dev/null; then
    hit_top3=1
    pass_top3=$((pass_top3 + 1))
  fi

  short_query=$(echo "$query" | cut -c1-53)
  if [[ $hit_top1 -eq 1 ]]; then t1="PASS"; else t1="FAIL"; fi
  if [[ $hit_top3 -eq 1 ]]; then t3="PASS"; else t3="FAIL"; fi
  printf "%-55s | %-8s | %-8s | %s\n" "$short_query" "$t1" "$t3" "$expected"
done

echo ""
echo "=== Results ==="
echo "Total queries: $total"
echo "Top-1 accuracy: $pass_top1/$total ($(echo "scale=1; $pass_top1 * 100 / $total" | bc)%)"
echo "Top-3 accuracy: $pass_top3/$total ($(echo "scale=1; $pass_top3 * 100 / $total" | bc)%)"
