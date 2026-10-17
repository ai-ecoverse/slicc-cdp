import { appendFileSync } from 'node:fs';
import { args, cleanEnv, gh, name, repo, run, save, semver } from './common.mjs';

const options = args({ version: { type: 'string' }, wait: { type: 'string', default: '3600' } });
if (!semver.test(options.version ?? '')) throw new Error('exact version required');
const seconds = Number(options.wait);
if (!Number.isSafeInteger(seconds) || seconds < 0 || seconds > 3600)
  throw new Error('invalid registry wait');
const until = Date.now() + seconds * 1000;
let metadata;
for (;;) {
  try {
    metadata = JSON.parse(
      run('npm', ['view', `${name}@${options.version}`, '--json', '--prefer-online'], {
        env: cleanEnv(),
      })
    );
    if (metadata.version === options.version && metadata.dist?.tarball) {
      const response = await fetch(metadata.dist.tarball, {
        method: 'HEAD',
        signal: AbortSignal.timeout(60000),
      });
      if (response.ok) break;
    }
  } catch (error) {
    if (!/E404|No match found/.test(`${error.stderr ?? ''}`)) throw error;
  }
  if (Date.now() >= until) throw new Error('INCOMPLETE: exact npm version unavailable');
  await new Promise((resolve) => setTimeout(resolve, 15000));
}
let tag = gh(`repos/${repo}/git/ref/tags/v${options.version}`).object;
for (let depth = 0; tag.type === 'tag' && depth < 10; depth++)
  tag = gh(`repos/${repo}/git/tags/${tag.sha}`).object;
if (tag.type !== 'commit' || !/^[a-f0-9]{40}$/.test(tag.sha))
  throw new Error('INCOMPLETE: tag commit unavailable');
const plan = {
  version: options.version,
  mode: 'release',
  legacyChecksum: false,
  postPublish: true,
  commit: tag.sha,
  harness: tag.sha,
  run: process.env.GITHUB_RUN_ID ?? 'local',
  attempt: process.env.GITHUB_RUN_ATTEMPT ?? '1',
};
save(options.out, plan);
console.log(`post-plan: PASS ${plan.version} ${options.out}`);
if (process.env.GITHUB_OUTPUT)
  appendFileSync(process.env.GITHUB_OUTPUT, `version=${plan.version}\n`);
