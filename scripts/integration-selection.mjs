import { readdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const redirectFiles = [
  'test/integration/guard-c2-redirect.test.mjs',
  'test/integration/redirect-navigation.test.mjs',
];

export const guardFiles = ['test/integration/guard-runner.test.mjs'];

export function integrationFiles(files, group = 'all') {
  if (!['all', 'integration', 'redirects', 'guard'].includes(group))
    throw new Error(`unknown integration job ${group}`);
  if (new Set(files).size !== files.length) throw new Error('duplicate integration test file');
  if (redirectFiles.some((file) => !files.includes(file)))
    throw new Error('required redirect regression file missing');
  if (guardFiles.some((file) => !files.includes(file)))
    throw new Error('required guard regression file missing');
  return files
    .filter((file) => {
      if (group === 'all') return true;
      if (group === 'guard') return guardFiles.includes(file);
      if (group === 'redirects') return redirectFiles.includes(file);
      return !redirectFiles.includes(file) && !guardFiles.includes(file);
    })
    .sort();
}

export async function integrationArguments(group = process.env.SLICC_INTEGRATION_JOB ?? 'all') {
  const directory = fileURLToPath(new URL('../test/integration/', import.meta.url));
  const files = (await readdir(directory))
    .filter((file) => file.endsWith('.test.mjs'))
    .map((file) => `test/integration/${file}`);
  return integrationFiles(files, group);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  console.log((await integrationArguments()).join(' '));
