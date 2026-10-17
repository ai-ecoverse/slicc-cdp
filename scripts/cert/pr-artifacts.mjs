import { createHash } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { cliVerdict } from './cli-verdict.mjs';
import { args, json, save } from './common.mjs';
import { artifactFindings, artifactInventory } from './pr-artifact-binding.mjs';
import { client } from './pr-common.mjs';

const printVerdict = cliVerdict('pr-artifacts: ');
const options = args({
  bindings: { type: 'string' },
  roles: { type: 'string', default: '[]' },
  phase: { type: 'string', default: 'consumer' },
  action: { type: 'string', default: 'inspect' },
  directory: { type: 'string' },
});
const plan = json(options.plan);
let result;
try {
  const { api } = client(plan.repository);
  const inventory = await artifactInventory(api, plan.run);
  let bindings;
  let bindingError;
  try {
    bindings = json(options.bindings);
  } catch (error) {
    bindingError = error.message;
  }
  result = artifactFindings(plan, bindings, inventory, {
    roles: JSON.parse(options.roles),
    phase: options.phase,
  });
  if (bindingError) result.incomplete.push(bindingError);
  result.verifiedBytes = [];
  if (!['inspect', 'download'].includes(options.action)) {
    result.failures.push('unknown artifact action');
    result.status = 'FAIL';
  }
  if (result.status === 'PASS' && options.action === 'download') {
    if (!options.directory) throw new Error('exact artifact archive directory required');
    for (const item of result.matched) {
      const origin = process.env.GITHUB_API_URL ?? 'https://api.github.com';
      const response = await fetch(
        `${origin}/repos/${plan.repository}/actions/artifacts/${item.id}/zip`,
        {
          headers: process.env.GH_TOKEN ? { Authorization: `Bearer ${process.env.GH_TOKEN}` } : {},
          signal: AbortSignal.timeout(60000),
        }
      );
      if (!response.ok) throw new Error(`artifact archive HTTP ${response.status}: ${item.role}`);
      const chunks = [];
      let bytes = 0;
      const hash = createHash('sha256');
      for await (const chunk of response.body) {
        bytes += chunk.length;
        if (bytes > 128 * 1024 * 1024) throw new Error('artifact archive exceeds bounded input');
        hash.update(chunk);
        chunks.push(chunk);
      }
      if (hash.digest('hex') !== item.digest) {
        result.failures.push(`artifact ZIP bytes mismatch: ${item.role}`);
        result.status = 'FAIL';
        break;
      }
      mkdirSync(options.directory, { recursive: true });
      const file = path.resolve(options.directory, `${item.role}.zip`);
      writeFileSync(file, Buffer.concat(chunks));
      result.verifiedBytes.push({ ...item, file, bytes });
    }
  }
} catch (error) {
  result ??= { which: 'artifacts', failures: [], incomplete: [] };
  result.incomplete.push(error.message);
  result.status = result.failures.length ? 'FAIL' : 'INCOMPLETE';
}
save(options.out, result);
if (options.action === 'download' && result.status !== 'PASS') process.exitCode = 1;
printVerdict(result.status, options.out);
