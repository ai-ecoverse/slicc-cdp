import { copyFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { cliVerdict } from './cli-verdict.mjs';
import { args, digest, json, save } from './common.mjs';
import { prIdentity } from './pr-common.mjs';

const printVerdict = cliVerdict('pr-sign-output: ');
const options = args({ directory: { type: 'string' } });
const plan = json(options.plan);
const input = json(path.join(options.directory, 'sign-input.json'));
if (JSON.stringify(prIdentity(input)) !== JSON.stringify(prIdentity(plan)))
  throw new Error('sign input identity mismatch');
mkdirSync(options.out, { recursive: true });
const files = input.sources
  .flatMap((source) => source.files)
  .map((row) => {
    if (!/^(playwright-cli|curlwright)-darwin-(amd64|arm64)$/.test(row.file))
      throw new Error('unexpected signer inventory');
    copyFileSync(path.join(options.directory, row.file), path.join(options.out, row.file));
    return { file: row.file, input: row.sha256, sha256: digest(path.join(options.out, row.file)) };
  });
if (files.length !== 4 || new Set(files.map((row) => row.file)).size !== 4)
  throw new Error('all four signature outputs required');
save(path.join(options.out, 'signed.json'), { ...prIdentity(plan), files });
printVerdict('PASS', options.out);
