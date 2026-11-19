#!/usr/bin/env bash
# Build loa-scenarios-<VERSION>.tar.gz (+ .sha256) from scenarios/.
# Entries are relative paths without "./" (loa rejects any other path form).
set -euo pipefail
out="${1:-dist}"
root="$(cd "$(dirname "$0")/.." && pwd)"
ver="$(tr -d '[:space:]' < "$root/scenarios/VERSION")"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
name="loa-scenarios-${ver}.tar.gz"
(
  cd "$root/scenarios"
  find . -type f \( -name '*.yaml' -o -name VERSION \) | sed 's|^\./||' | LC_ALL=C sort > "$out/.files"
  tar -czf "$out/$name" -T "$out/.files"
)
rm -f "$out/.files"
(cd "$out" && { command -v sha256sum >/dev/null && sha256sum "$name" || shasum -a 256 "$name"; } > "$name.sha256")
echo "$out/$name"
