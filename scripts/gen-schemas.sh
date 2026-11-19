#!/usr/bin/env bash
# Regenerate schemas/*.schema.json from the code (loa schema <document>).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
cargo build -q -p loa-cli
for t in loa.yaml scenario output error task; do
  ./target/debug/loa --json schema "$t" \
    | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin)['data'], ensure_ascii=False, indent=2))" \
    > "schemas/$t.schema.json"
done
