import { lstatSync, readFileSync, realpathSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { save } from './common.mjs';
import { guardSuite } from './guard-suite.mjs';
import { suite } from './suite.mjs';
import { guardFile } from './suite-worker.mjs';

export async function integrationSuite(options, execute = suite, executeGuard = guardSuite) {
  if (!options.files.includes(guardFile)) return execute(options);
  try {
    const require = createRequire(path.join(realpathSync(options.cwd), 'package.json'));
    const installed = realpathSync(path.join(options.cwd, 'node_modules'));
    const file = require.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json');
    if (!file.startsWith(installed + path.sep))
      throw new Error('guard corpus resolved outside the staged installation');
    const stat = lstatSync(file);
    if (!stat.isFile() || stat.size > 32 * 1024 * 1024)
      throw new Error('staged guard corpus must be a bounded regular file');
    const vectors = JSON.parse(readFileSync(file, 'utf8'));
    return await executeGuard({ ...options, vectors, runSuite: execute });
  } catch (error) {
    const result = {
      status: error.certStatus ?? 'INCOMPLETE',
      reason: error.message,
      files: options.files,
      missing: options.files,
      rows: [],
      attempts: [],
      ...(error.certStatus === 'FAIL' ? { failureKind: 'integrity' } : {}),
    };
    save(`${options.out}.json`, result);
    return result;
  }
}
