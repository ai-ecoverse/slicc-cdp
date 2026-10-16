import { parseArgs } from 'node:util';
import { capture, root, save } from './common.mjs';

const { values } = parseArgs({
  options: {
    directory: { type: 'string', default: root },
    commit: { type: 'string' },
    mode: { type: 'string', default: 'release' },
    cutoff: { type: 'string', default: 'dab966a0b357b83f21479080152407414c83e05a' },
    out: { type: 'string' },
  },
});
const ancestry = capture('git', ['merge-base', '--is-ancestor', values.cutoff, values.commit], {
  cwd: values.directory,
});
const legacy = values.mode === 'replay' && ancestry.exit === 1;
const result = {
  legacy,
  mode: values.mode,
  cutoff: values.cutoff,
  commit: values.commit,
  ancestry: ancestry.exit,
};
if (values.out) save(values.out, result);
console.log(JSON.stringify(result));
