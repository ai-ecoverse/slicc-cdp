#!/usr/bin/env bash
set -euo pipefail

outfile="${1:-THIRD-PARTY-NOTICES}"
targets="${2:-}"

header="THIRD-PARTY SOFTWARE NOTICES AND INFORMATION

This project incorporates material from the projects listed below.
"

collect_deps() {
  local extra_args=()
  if [ -n "$targets" ]; then
    for t in $targets; do
      extra_args+=(--filter-platform "$t")
    done
  fi
  cargo metadata --format-version 1 --locked "${extra_args[@]}" 2>/dev/null \
    | node -e "
      const m = JSON.parse(require('fs').readFileSync('/dev/stdin', 'utf8'));
      const ws = new Set(m.workspace_members);
      const seen = new Set();
      for (const p of m.packages) {
        if (ws.has(p.id)) continue;
        const key = p.name + '@' + p.version;
        if (seen.has(key)) continue;
        seen.add(key);
        const license = p.license || 'UNKNOWN';
        const repo = (p.repository || p.homepage || '');
        console.log(JSON.stringify({ name: p.name, version: p.version, license, repo }));
      }
    "
}

deps="$(collect_deps)"

{
  printf '%s\n\n' "$header"
  echo "$deps" | node -e "
    const lines = require('fs').readFileSync('/dev/stdin', 'utf8').trim().split('\n').filter(Boolean);
    const deps = lines.map(l => JSON.parse(l));
    deps.sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
    const seen = new Set();
    for (const d of deps) {
      const key = d.name + '@' + d.version;
      if (seen.has(key)) continue;
      seen.add(key);
      let out = '== ' + d.name + ' ' + d.version + ' ==\n';
      out += 'License: ' + d.license + '\n';
      if (d.repo) out += 'Repository: ' + d.repo + '\n';
      out += '\n';
      process.stdout.write(out);
    }
  "
} > "$outfile"

echo "wrote $outfile ($(wc -l < "$outfile") lines)"
