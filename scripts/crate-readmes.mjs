import { readFileSync, realpathSync, writeFileSync } from 'node:fs';
import path from 'node:path';

export const CRATES = [
  ['slicc-cdp-client', 'cdp-client'],
  ['slicc-cdp', 'playwright-cli'],
  ['curlwright', 'curlwright'],
];

const root = path.resolve(import.meta.dirname, '..');

export function crateReadme(readme, name) {
  const crates = readme.split('\n## Crates\n')[1];
  if (!crates) throw new Error('README.md has no ## Crates section');
  const section = crates.split(/\n## /)[0].split(`\n### ${name}\n`)[1];
  if (!section) throw new Error(`README.md ## Crates has no ### ${name}`);
  let fence = false;
  const lines = section
    .split(/\n### /)[0]
    .split('\n')
    .map((line) => {
      if (line.startsWith('```')) fence = !fence;
      return !fence && /^#{4,} /.test(line) ? line.slice(2) : line;
    });
  return `# ${name}\n${lines.join('\n').trimEnd()}\n`;
}

export function writeCrateReadmes(readme = readFileSync(path.join(root, 'README.md'), 'utf8')) {
  for (const [name, directory] of CRATES) {
    writeFileSync(path.join(root, 'crates', directory, 'README.md'), crateReadme(readme, name));
  }
  return CRATES.length;
}

const invoked = (() => {
  try {
    return realpathSync(process.argv[1]) === realpathSync(import.meta.filename);
  } catch {
    return false;
  }
})();

if (invoked) {
  console.log(`crate-readmes: PASS ${writeCrateReadmes()} READMEs`);
}
