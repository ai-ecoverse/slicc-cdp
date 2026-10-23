import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import { test } from 'node:test';

const read = (path) => readFile(new URL(`../../${path}`, import.meta.url), 'utf8');
const ci = await read('.github/workflows/ci.yml');
const pkg = JSON.parse(await read('package.json'));
const sliccy = JSON.parse(await read('packages/sliccy-browser/package.json'));
const pipeline = ci.split('\n  released:\n')[0];
const input = (name) => pipeline.match(new RegExp(`\\n {6}${name}: (.+)\\n`))?.[1];
const job = (name) => ci.match(new RegExp(`\\n {2}${name}:\\n((?: {4}.*\\n|\\n)+)`))?.[1] ?? '';

test('CI is one caller of slicc-shared-web pipeline.yml, at the release of the harness the tests use', async () => {
  assert.deepEqual(await readdir(new URL('../../.github/workflows/', import.meta.url)), ['ci.yml']);
  assert.deepEqual(await readdir(new URL('../../.github/', import.meta.url)), ['workflows']);
  const version = pkg.devDependencies['@ai-ecoverse/slicc-shared-web'];
  assert.match(version, /^2\.\d+\.\d+$/);
  assert.match(
    ci,
    new RegExp(
      `\\njobs:\\n {2}ci:\\n {4}uses: ai-ecoverse/slicc-shared-web/\\.github/workflows/pipeline\\.yml@v${version.replaceAll('.', '\\.')}\\n {4}secrets: inherit\\n {4}with:\\n`
    )
  );
  assert.equal(ci.match(/uses: ai-ecoverse\/slicc-shared-web\//g).length, 1);
  assert.doesNotMatch(ci, /uses: \.\//);
  assert.deepEqual(ci.split('\njobs:\n')[1].match(/^ {2}[a-z-]+:$/gm), [
    '  ci:',
    '  released:',
    '  crates:',
    '  binaries:',
    '  assets:',
    '  sliccy-browser:',
  ]);
});

test('CI runs on pull requests, merge groups and pushes to main, and cancels a superseded run only on a pull request', () => {
  assert.match(
    ci,
    /\non:\n {2}pull_request:\n {2}merge_group:\n {2}push:\n {4}branches: \[main\]\n\n/
  );
  assert.match(
    ci,
    /\nconcurrency:\n {2}group: ci-\$\{\{ github\.ref \}\}\n {2}cancel-in-progress: \$\{\{ github\.event_name == 'pull_request' \}\}\n/
  );
  assert.doesNotMatch(ci, /workflow_dispatch|schedule:|workflow_run|workflow_call/);
  for (const permission of ['contents: write', 'attestations: write', 'id-token: write'])
    assert.ok(ci.split('\njobs:\n')[0].includes(`\n  ${permission}\n`), permission);
});

test('the pipeline picks its runners, reviews no Renovate pull request and runs nothing after its release', () => {
  for (const name of [
    'runs-on',
    'after-release',
    'review-renovate',
    'release-environment',
    'deploy',
    'node-options',
  ]) {
    assert.equal(input(name), undefined, name);
  }
  assert.doesNotMatch(ci, /vars\.|CERT_|cert-review/);
  assert.equal(input('shards'), '8');
  assert.equal(input('timeout-minutes'), '30');
  assert.match(input('review-prompt'), /^\|$/);
});

test('the shards split exactly the files npm test runs, on the Node the package asks for', async () => {
  assert.equal(input('test'), undefined);
  assert.equal(input('test-glob'), 'test/integration/*.test.mjs');
  assert.ok(pkg.scripts.test.endsWith(' node scripts/test-integration.mjs'), pkg.scripts.test);
  const entrypoint = await read('scripts/test-integration.mjs');
  assert.ok(entrypoint.includes(`'${input('test-glob')}'`), entrypoint);
  assert.match(
    entrypoint,
    /import \{ testArguments \} from '\.\.\/test\/integration\/test-arguments\.mjs'/
  );
  assert.match(entrypoint, /spawnSync\(process\.execPath, \[\.\.\.testArguments, \.\.\.files\]/);
  const argumentsSource = await read('test/integration/test-arguments.mjs');
  assert.match(argumentsSource, /'--import'/);
  assert.match(argumentsSource, /'\.\/ports\.mjs'/);
  assert.match(argumentsSource, /'--test'/);
  assert.match(argumentsSource, /'--test-timeout=600000'/);
  assert.doesNotMatch(pkg.scripts.test, /NODE_OPTIONS/);
  assert.equal(input('node-version'), undefined);
  assert.equal(pkg.engines.node, '>=24');
  assert.match(pkg.scripts.pretest, /cargo build --locked -p slicc-cdp -p curlwright/);
  assert.match(pkg.scripts.pretest, /--target wasm32-wasip1/);
  assert.match(pkg.scripts.pretest, /cargo build --locked -p slicc-cdp-client --examples/);
  assert.match(
    pkg.scripts.pretest,
    /--target wasm32-wasip1 -p slicc-cdp-client --example guard_bridge/
  );
  assert.match(pkg.scripts.pretest, /npm run -s browser/);
  assert.match(pkg.scripts.browser, /install-browser --version "\$npm_package_config_chrome"/);
  assert.equal(pkg.scripts['test:integration'], undefined);
  for (const name of ['playwright', 'playwright-core', 'puppeteer', 'puppeteer-core'])
    assert.equal(pkg.devDependencies[name], undefined, name);
});

test('semantic-release publishes to npm from the trusted ci.yml, with provenance, into a draft release', async () => {
  assert.equal(pkg.fledgling.workflow, 'ci.yml');
  assert.equal(pkg.fledgling.repo, 'ai-ecoverse/slicc-cdp');
  assert.equal(pkg.publishConfig.provenance, true);
  assert.equal(pkg.scripts.prepublishOnly, 'npm run build:wasm');
  const release = JSON.parse(await read('.releaserc.json'));
  assert.deepEqual(release.branches, ['main']);
  assert.ok(release.plugins.includes('@semantic-release/npm'));
  assert.doesNotMatch(JSON.stringify(release), /npmPublish/);
  assert.deepEqual(release.plugins.at(-1), ['@semantic-release/github', { draftRelease: true }]);
});

test('the crates, binaries and release assets follow only a release on main', () => {
  assert.match(job('released'), /needs: ci\n/);
  assert.match(
    job('released'),
    /if: github\.event_name == 'push' && github\.ref == 'refs\/heads\/main'\n/
  );
  for (const name of ['crates', 'binaries']) {
    assert.match(job(name), /needs: released\n/, name);
    assert.match(job(name), /if: needs\.released\.outputs\.version != ''\n/, name);
  }
  assert.match(job('assets'), /needs: \[released, binaries\]\n/);
  assert.match(job('crates'), /id-token: write/);
  assert.match(job('crates'), /run: cargo fetch --locked\n[\s\S]*scripts\/publish-crates\.sh/);
  assert.match(job('binaries'), /scripts\/sign-macos\.sh/);
  assert.match(job('binaries'), /source=Notarized Developer ID/);
  assert.doesNotMatch(
    job('binaries'),
    /\| *(tee|grep -q)\b/,
    'grep -q and tee in a pipe can kill its writer with SIGPIPE under pipefail'
  );
  for (const label of [
    'darwin-amd64',
    'darwin-arm64',
    'linux-amd64',
    'linux-arm64',
    'windows-amd64',
    'windows-arm64',
  ])
    assert.match(job('binaries'), new RegExp(`label: ${label} `), label);
  assert.match(job('assets'), /release-checksums\.mjs/);
  assert.match(job('assets'), /--draft=false --latest="\$latest"/);
  assert.match(job('assets'), /\[ "\$TAG" = "\$highest" \] && latest=true/);
  assert.match(job('assets'), /git ls-remote --tags --refs origin 'refs\/tags\/v\*'/);
  assert.doesNotMatch(
    ci,
    /grep -q/,
    'grep -q in a pipe can kill its writer with SIGPIPE under pipefail'
  );
  for (const name of ['released', 'crates', 'binaries', 'assets', 'sliccy-browser'])
    assert.match(job(name), /\n {4}permissions:\n/, name);
});

test('sliccy-browser is published from ci.yml when its version is new, with provenance', () => {
  assert.equal(sliccy.publishConfig.provenance, true);
  assert.match(job('sliccy-browser'), /needs: ci\n/);
  assert.match(
    job('sliccy-browser'),
    /if: github\.event_name == 'push' && github\.ref == 'refs\/heads\/main'\n/
  );
  assert.match(job('sliccy-browser'), /working-directory: packages\/sliccy-browser\n/);
  assert.match(job('sliccy-browser'), /npm view "\$name@\$version" version/);
  assert.match(job('sliccy-browser'), /id-token: write/);
});

test('the workflow has no comments but renovate annotations', () => {
  for (const line of ci.split('\n')) {
    if (/^\s*#/.test(line)) assert.match(line, /^\s*# renovate:/);
  }
});

test('Renovate holds majors and .github/ updates as drafts, has no certification rules, and leaves slicc-shared-web to the shared preset', async () => {
  const config = JSON.parse(await read('renovate.json'));
  assert.deepEqual(config.extends, ['github>ai-ecoverse/slicc-shared-web']);
  const held = (match) =>
    config.packageRules.find(
      (rule) => JSON.stringify(rule).includes(match) && rule.automerge === false
    );
  assert.equal(held('"major"')?.draftPR, true);
  assert.equal(held('".github/**"')?.draftPR, true);
  assert.doesNotMatch(JSON.stringify(config), /certif|cert-review|platformAutomerge/);
  assert.equal(
    config.packageRules.some((rule) =>
      JSON.stringify(rule).includes('ai-ecoverse/slicc-shared-web')
    ),
    false
  );
});
