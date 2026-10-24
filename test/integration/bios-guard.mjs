import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const head = '39c2e7ca244ddad058d3e978ae712be61c77f387';
const sources = {
  'guard.js': 'f6d97cefbc88380b3c9268a993c9ea73131663a0292074efcb28e97b242a539b',
  'held.js': '27cf0b90c01c0c4cdbd23139cf82c7814d3a2d398de7f69972dba0be7af067b1',
};

export async function biosGuard() {
  const dir = path.resolve('node_modules/.cache', `bios-guard-${head}`);
  await mkdir(dir, { recursive: true });
  await writeFile(path.join(dir, 'package.json'), '{"type":"module"}\n');
  for (const [name, sum] of Object.entries(sources)) {
    const file = path.join(dir, name);
    let source = await readFile(file).catch(() => null);
    if (!source) {
      const url = `https://raw.githubusercontent.com/ai-ecoverse/slicc-bios/${head}/src/seed/${name}`;
      const response = await fetch(url, { signal: AbortSignal.timeout(30000) });
      assert.equal(response.status, 200, url);
      source = Buffer.from(await response.arrayBuffer());
    }
    assert.equal(createHash('sha256').update(source).digest('hex'), sum, name);
    await writeFile(file, source);
  }
  await writeFile(
    path.join(dir, 'adobe.js'),
    "export const RELAY = 'https://www.sliccy.ai/auth/callback';\nexport const SIGN_IN_CLIENTS = new Set(['experience-catalyst-prod']);\n"
  );
  const { guardProtected } = await import(pathToFileURL(path.join(dir, 'guard.js')).href);
  return (connection) => guardProtected(connection, 'https://seven.sliccy.ai');
}
