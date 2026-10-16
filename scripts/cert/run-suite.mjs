import { parseArgs } from 'node:util';
import { suite } from './suite.mjs';

const { values } = parseArgs({
  options: {
    directory: { type: 'string' },
    files: { type: 'string' },
    out: { type: 'string' },
    retry: { type: 'boolean', default: true },
  },
  allowNegative: true,
});
const result = await suite({
  cwd: values.directory,
  files: JSON.parse(values.files),
  out: values.out,
  retry: values.retry,
});
console.log(JSON.stringify(result));
process.exitCode = result.status === 'PASS' ? 0 : 1;
