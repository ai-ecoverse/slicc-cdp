import { existsSync, readFileSync, writeFileSync } from 'node:fs';

const path = 'node_modules/@ai-ecoverse/slicc-shared-web/bin/slicc-lint-comments.sh';
if (!existsSync(path)) process.exit(0);
const needle = "'^ok: |^(.+/)?AGENTS\\.md: documentation file is not allowed'";
const extra =
  '|^crates/(cdp-client|playwright-cli|curlwright)/README\\.md: documentation file is not allowed';
const text = readFileSync(path, 'utf8');
if (text.includes(extra)) process.exit(0);
if (!text.includes(needle)) {
  process.stderr.write('slicc-lint-comments allowlist no longer matches\n');
  process.exit(1);
}
writeFileSync(path, text.replace(needle, needle.slice(0, -1) + extra + "'"));
