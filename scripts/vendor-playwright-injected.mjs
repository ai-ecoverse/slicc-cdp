import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';

const version = process.argv[2] ?? '1.64.0-alpha-1790635538000';
const esbuild = 'esbuild@0.25.10';
const root = path.resolve(import.meta.dirname, '..');
const vendor = path.join(root, 'crates/playwright-cli/vendor');
const target = path.join(vendor, 'playwright-injected.js.txt');
const scratch = mkdtempSync(path.join(tmpdir(), 'slicc-playwright-injected-'));
const header = [
  '/**',
  ' * Copyright (c) Microsoft Corporation.',
  ' *',
  ' * Licensed under the Apache License, Version 2.0 (the "License");',
  ' * you may not use this file except in compliance with the License.',
  ' * You may obtain a copy of the License at',
  ' *',
  ' * http://www.apache.org/licenses/LICENSE-2.0',
  ' *',
  ' * Unless required by applicable law or agreed to in writing, software',
  ' * distributed under the License is distributed on an "AS IS" BASIS,',
  ' * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.',
  ' * See the License for the specific language governing permissions and',
  ' * limitations under the License.',
  ' */',
  '/**',
  ` * Playwright injected script from playwright-core@${version}, lib/coreBundle.js`,
  ` * (packages/playwright-core/src/generated/injectedScriptSource.ts), minified with ${esbuild}`,
  ' * by scripts/vendor-playwright-injected.mjs. See LICENSE and NOTICE in this directory.',
  ' */',
  '',
].join('\n');

try {
  const tarball = execFileSync('npm', ['pack', `playwright-core@${version}`, '--silent'], {
    cwd: scratch,
    encoding: 'utf8',
  }).trim();
  execFileSync('tar', ['xzf', tarball], { cwd: scratch });
  const bundle = readFileSync(path.join(scratch, 'package/lib/coreBundle.js'), 'utf8');
  const marker = 'packages/playwright-core/src/generated/injectedScriptSource.ts"() {';
  const block = bundle.indexOf(marker);
  if (block < 0) throw new Error('injectedScriptSource was not found in coreBundle.js');
  const start = bundle.indexOf("'", block);
  let end = start + 1;
  while (bundle[end] !== "'") end += bundle[end] === '\\' ? 2 : 1;
  const source = new Function(`return ${bundle.slice(start, end + 1)};`)();
  writeFileSync(
    path.join(scratch, 'entry.js'),
    `const module = {};\n${source}\nexport const sliccInjectedScriptClass = module.exports.InjectedScript();\n`
  );
  execFileSync(
    'npx',
    [
      '--yes',
      esbuild,
      'entry.js',
      '--minify',
      '--format=iife',
      '--global-name=sliccInjected',
      '--charset=utf8',
      '--legal-comments=none',
      '--target=es2022',
      '--outfile=entry.min.js',
    ],
    { cwd: scratch, stdio: 'inherit' }
  );
  writeFileSync(target, header + readFileSync(path.join(scratch, 'entry.min.js'), 'utf8'));
  for (const name of ['LICENSE', 'NOTICE'])
    writeFileSync(path.join(vendor, name), readFileSync(path.join(scratch, 'package', name)));
  process.stdout.write(`wrote ${path.relative(root, target)} from playwright-core@${version}\n`);
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
