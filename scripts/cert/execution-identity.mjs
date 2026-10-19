import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { lstatSync, mkdirSync, readFileSync as readFile } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { root, run } from './common.mjs';
import { integrity } from './pr-subject.mjs';

const readFileSync = (file) => {
  const stat = lstatSync(file);
  if (!stat.isFile() || stat.size > 32 * 1024 * 1024)
    throw new Error('execution identity input must be a regular file no larger than 32 MiB');
  return readFile(file);
};
const compare = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const sha = /^[a-f0-9]{40}$/;
const version = /^\d+\.\d+\.\d+$/;
const crates = ['cdp-client', 'discovery', 'playwright-cli', 'curlwright'];
const cache = new Map();
const sources = new Map();

export function corpusIdentity(directory) {
  const bytes = readFileSync(path.join(directory, 'guard-vectors.json'));
  const vectors = JSON.parse(bytes);
  const manifest = JSON.parse(readFileSync(path.join(directory, 'package.json'), 'utf8'));
  if (
    manifest.name !== '@ai-ecoverse/slicc-browsers' ||
    !version.test(manifest.version) ||
    vectors.setVersion !== manifest.version
  )
    throw new Error('guard corpus package/version unavailable');
  const keys = vectors.vectors
    .flatMap((vector) =>
      vector.targets.flatMap((target) =>
        Object.keys(vectors.variants).map((variant) => `${vector.id}/${target}/${variant}/cdp`)
      )
    )
    .sort();
  if (!keys.length || new Set(keys).size !== keys.length)
    throw integrity('guard corpus row universe ambiguous');
  return {
    version: manifest.version,
    sha256: hash(bytes),
    driverSha256: hash(readFileSync(path.join(directory, 'guard-driver.mjs'))),
    runnerSha256: hash(readFileSync(path.join(directory, 'guard-runner.mjs'))),
    universeSha256: hash(JSON.stringify(keys)),
    rows: keys.length,
  };
}

export function installedCorpus(directory) {
  const require = createRequire(path.join(directory, 'package.json'));
  return corpusIdentity(
    path.dirname(require.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json'))
  );
}

function cargo(bytes, file) {
  const section = bytes
    .toString('utf8')
    .split(/^\[package\]\s*$/m)[1]
    ?.split(/^\[/m)[0];
  const values = [...(section ?? '').matchAll(/^version\s*=\s*"([^"]+)"\s*$/gm)];
  if (values.length !== 1 || !version.test(values[0][1]))
    throw new Error(`product Cargo version unavailable: ${file}`);
  return { file, version: values[0][1], sha256: hash(bytes) };
}

export function sourceIdentity(commit, directory = root) {
  if (!sha.test(commit ?? '')) throw new Error('execution source commit unavailable');
  const key = `${directory}/${commit}`;
  if (sources.has(key)) return sources.get(key);
  const read = (file) =>
    execFileSync('git', ['show', `${commit}:${file}`], {
      cwd: directory,
      timeout: 120000,
      maxBuffer: 32 * 1024 * 1024,
    });
  const files = run('git', ['ls-tree', '-r', '--name-only', commit, '--', 'test'], {
    cwd: directory,
  })
    .split('\n')
    .filter(Boolean)
    .sort();
  if (!files.length) throw new Error('execution harness source unavailable');
  const manifest = JSON.parse(read('package.json'));
  const lock = JSON.parse(read('package-lock.json'));
  const pin = manifest.devDependencies?.['@ai-ecoverse/slicc-browsers'];
  const entry = lock.packages?.['node_modules/@ai-ecoverse/slicc-browsers'];
  if (
    !version.test(pin ?? '') ||
    entry?.version !== pin ||
    !/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(entry.integrity ?? '') ||
    entry.resolved !==
      `https://registry.npmjs.org/@ai-ecoverse/slicc-browsers/-/slicc-browsers-${pin}.tgz`
  )
    throw new Error('execution corpus exact locked package unavailable');
  const source = {
    commit,
    cargo: crates.map((crate) => {
      const file = `crates/${crate}/Cargo.toml`;
      return cargo(read(file), file);
    }),
    harness: files.map((file) => ({ file, sha256: hash(read(file)) })),
    corpus: { version: pin, integrity: entry.integrity },
  };
  sources.set(key, source);
  return source;
}

export function stagedIdentity(directory, subject, harness, sha256) {
  for (const item of subject.cargo) {
    if (!isDeepStrictEqual(cargo(readFileSync(path.join(directory, item.file)), item.file), item))
      throw integrity(`staged product Cargo/source differs: ${item.file}`);
  }
  const files = harness.harness.map(({ file }) => ({
    file,
    sha256: hash(readFileSync(path.join(directory, file))),
  }));
  if (!isDeepStrictEqual(files, harness.harness))
    throw integrity('staged harness differs from exact source');
  const corpus = installedCorpus(directory);
  if (corpus.version !== harness.corpus.version)
    throw integrity('staged corpus differs from locked harness');
  return {
    schema: 1,
    subject: { commit: subject.commit, sha256, cargo: subject.cargo },
    harness: { commit: harness.commit },
    corpus: { ...corpus, integrity: harness.corpus.integrity },
  };
}

export function effectiveHarness(harness, tools, scope) {
  const files = new Map(harness.harness.map((item) => [item.file, item.sha256]));
  files.set(
    '@main/src/browser.js',
    hash(execFileSync('git', ['show', `${tools.commit}:src/browser.js`], { cwd: root }))
  );
  if (scope === 'proof-integration') {
    for (const item of tools.harness) {
      if (
        (item.file.endsWith('.mjs') && !item.file.endsWith('.test.mjs')) ||
        item.file.endsWith('.rs')
      )
        files.set(item.file, item.sha256);
    }
  }
  return [...files].sort(([a], [b]) => compare(a, b)).map(([file, sha256]) => ({ file, sha256 }));
}

export function observeIdentity(directory, expected, scope, toolsCommit, proofFiles = []) {
  if (
    expected?.schema !== 1 ||
    !sha.test(expected.subject?.commit ?? '') ||
    !sha.test(expected.harness?.commit ?? '')
  )
    throw new Error('staged execution identity unavailable');
  const corpus = installedCorpus(directory);
  if (
    !isDeepStrictEqual(
      corpus,
      Object.fromEntries(Object.entries(expected.corpus).filter(([key]) => key !== 'integrity'))
    )
  )
    throw integrity('executed corpus/driver/universe differs from staged identity');
  for (const item of expected.subject.cargo) {
    if (!isDeepStrictEqual(cargo(readFileSync(path.join(directory, item.file)), item.file), item))
      throw integrity('executed product Cargo differs from staged identity');
  }
  const harness = sourceIdentity(expected.harness.commit);
  const tools = sourceIdentity(toolsCommit);
  const files = effectiveHarness(harness, tools, scope);
  if (scope === 'proof-integration') {
    for (const file of proofFiles) {
      const source = tools.harness.find((item) => item.file === file);
      if (!source) throw new Error('named proof source unavailable');
      const current = files.find((item) => item.file === file);
      if (current) current.sha256 = source.sha256;
      else files.push(source);
    }
  }
  if (scope === 'guard') {
    const file = 'crates/cdp-client/examples/guard_bridge.rs';
    files.push({
      file,
      sha256: hash(execFileSync('git', ['show', `${toolsCommit}:${file}`], { cwd: root })),
    });
  }
  files.sort((a, b) => compare(a.file, b.file));
  const observed = files.map(({ file }) => ({
    file,
    sha256: hash(
      readFileSync(
        file.startsWith('@main/') ? path.join(root, file.slice(6)) : path.join(directory, file)
      )
    ),
  }));
  if (!isDeepStrictEqual(observed, files))
    throw integrity('executed harness differs from exact source/overlay');
  return {
    ...expected,
    scope,
    harness: {
      commit: harness.commit,
      tools: toolsCommit,
      overlay: scope === 'proof-integration' ? toolsCommit : null,
      sha256: hash(JSON.stringify(files)),
    },
  };
}

export function referenceCorpus(pin) {
  const key = JSON.stringify(pin);
  if (cache.has(key)) return cache.get(key);
  if (!version.test(pin.version) || !/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(pin.integrity))
    throw new Error('reference corpus exact pin unavailable');
  const directory = path.join(root, '.cert/execution-corpus', pin.version);
  mkdirSync(directory, { recursive: true });
  const packed = JSON.parse(
    run('npm', [
      'pack',
      `@ai-ecoverse/slicc-browsers@${pin.version}`,
      '--ignore-scripts',
      '--json',
      '--pack-destination',
      directory,
    ])
  );
  if (packed.length !== 1 || !/^[\w.-]+\.tgz$/.test(packed[0].filename))
    throw new Error('reference corpus archive unavailable');
  const tarball = path.join(directory, packed[0].filename);
  const actual = `sha512-${createHash('sha512').update(readFileSync(tarball)).digest('base64')}`;
  if (actual !== pin.integrity)
    throw integrity('reference corpus archive differs from source lock');
  const entries = run('tar', ['-tzf', tarball]).split('\n');
  if (
    entries.some((file) => !file.startsWith('package/') || file.split('/').includes('..')) ||
    run('tar', ['-tvzf', tarball])
      .split('\n')
      .some((line) => !/^[d-]/.test(line))
  )
    throw integrity('unsafe reference corpus archive');
  if (
    [
      'package/package.json',
      'package/guard-vectors.json',
      'package/guard-driver.mjs',
      'package/guard-runner.mjs',
    ].some((name) => entries.filter((entry) => entry === name).length !== 1)
  )
    throw new Error('reference corpus canonical inputs absent or ambiguous');
  run('tar', ['-xzf', tarball, '-C', directory]);
  const identity = { ...corpusIdentity(path.join(directory, 'package')), integrity: pin.integrity };
  if (identity.version !== pin.version)
    throw integrity('reference corpus version differs from source lock');
  cache.set(key, identity);
  return identity;
}

export function expectedExecutionIdentity(
  subject,
  harness,
  tools,
  scope,
  sha256,
  proofFiles = [],
  corpus = referenceCorpus(harness.corpus)
) {
  const files = effectiveHarness(harness, tools, scope);
  if (scope === 'proof-integration') {
    for (const file of proofFiles) {
      const item = tools.harness.find((item) => item.file === file);
      if (!item) throw new Error('named proof source unavailable');
      const index = files.findIndex((item) => item.file === file);
      if (index < 0) files.push(item);
      else files[index] = item;
    }
  }
  if (scope === 'guard') {
    const file = 'crates/cdp-client/examples/guard_bridge.rs';
    files.push({
      file,
      sha256: hash(execFileSync('git', ['show', `${tools.commit}:${file}`], { cwd: root })),
    });
  }
  files.sort((a, b) => compare(a.file, b.file));
  return {
    schema: 1,
    subject: { commit: subject.commit, sha256, cargo: subject.cargo },
    scope,
    harness: {
      commit: harness.commit,
      tools: tools.commit,
      overlay: scope === 'proof-integration' ? tools.commit : null,
      sha256: hash(JSON.stringify(files)),
    },
    corpus,
  };
}

function completeIdentity(actual) {
  const digest = /^[a-f0-9]{64}$/;
  return !(
    actual?.schema !== 1 ||
    !sha.test(actual.subject?.commit ?? '') ||
    !digest.test(actual.subject?.sha256 ?? '') ||
    !Array.isArray(actual.subject?.cargo) ||
    actual.subject.cargo.length !== crates.length ||
    actual.subject.cargo.some(
      (item) =>
        !item ||
        typeof item.file !== 'string' ||
        !item.file ||
        !version.test(item.version ?? '') ||
        !digest.test(item.sha256 ?? '')
    ) ||
    !sha.test(actual.harness?.commit ?? '') ||
    !sha.test(actual.harness?.tools ?? '') ||
    !digest.test(actual.harness?.sha256 ?? '') ||
    !Object.hasOwn(actual.harness, 'overlay') ||
    !version.test(actual.corpus?.version ?? '') ||
    ['sha256', 'driverSha256', 'runnerSha256', 'universeSha256'].some(
      (key) => !digest.test(actual.corpus?.[key] ?? '')
    ) ||
    !Number.isInteger(actual.corpus?.rows) ||
    actual.corpus.rows < 1 ||
    !/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(actual.corpus.integrity ?? '') ||
    !['guard', 'integration', 'proof-integration'].includes(actual.scope)
  );
}

export function validateExecutionIdentity(execution, expected, label, emit) {
  const actual = execution?.identity;
  if (!completeIdentity(actual)) {
    emit('INCOMPLETE', `${label}: validated execution identity absent`);
    return;
  }
  if (!isDeepStrictEqual(actual, expected))
    emit('FAIL', `${label}: execution identity differs from independent source/corpus`);
  const reports = [
    execution.guardReport?.report,
    ...(Array.isArray(execution.attempts) ? execution.attempts : []).flatMap((attempt) =>
      (Array.isArray(attempt?.workers) ? attempt.workers : []).map(
        (worker) => worker?.guardReport?.report
      )
    ),
  ].filter(Boolean);
  for (const report of reports) {
    if (!sha.test(report.sha ?? '') || typeof report.set !== 'string')
      emit('INCOMPLETE', `${label}: guard report source/corpus absent`);
    else if (
      report.sha !== expected.subject.commit ||
      report.set !== `@ai-ecoverse/slicc-browsers@${expected.corpus.version}`
    )
      emit('FAIL', `${label}: guard report executed another source/corpus`);
  }
  const report = execution.guardReport?.report;
  if (execution.files?.includes('test/integration/guard-runner.test.mjs')) {
    const keys =
      Array.isArray(report?.results) &&
      report.results.every((row) => row && typeof row === 'object')
        ? report.results
            .map((row) => `${row.id}/${row.target}/${row.variant}/${row.transport}`)
            .sort()
        : [];
    if (
      keys.length !== expected.corpus.rows ||
      new Set(keys).size !== keys.length ||
      hash(JSON.stringify(keys)) !== expected.corpus.universeSha256
    )
      emit('INCOMPLETE', `${label}: guard report exact row universe absent`);
  }
}
