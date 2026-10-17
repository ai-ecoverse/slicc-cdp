import { appendFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { args, identity, json, name, repo, save } from './common.mjs';

const options = args({ evidence: { type: 'string' } });
const plan = json(path.join(options.evidence, 'plan.json'));
const failures = [];
const incomplete = [];
const check = (condition, message, available = true) => {
  if (!available) incomplete.push(message);
  else if (!condition) failures.push(message);
};
const documents = {};
for (const which of ['registry', 'assets-linux', 'assets-macos']) {
  try {
    const item = json(path.join(options.evidence, `${which}.json`));
    if (JSON.stringify(identity(item)) !== JSON.stringify(identity(plan)) || item.which !== which)
      throw new Error('source/run identity mismatch');
    documents[which] = item;
    if (item.status !== 'PASS')
      (item.status === 'FAIL' ? failures : incomplete).push(
        `${which}: ${item.reason ?? item.status}`
      );
  } catch (error) {
    incomplete.push(`${which}: ${error.message}`);
  }
}
if (process.env.CERT_UPSTREAM) {
  const upstream = JSON.parse(process.env.CERT_UPSTREAM);
  for (const job of ['plan', 'assets'])
    if (upstream[job] !== 'success') incomplete.push(`${job}: ${upstream[job] ?? 'missing'}`);
}
const registry = documents.registry;
function verifyRegistry() {
  const statement = registry?.statement;
  const definition = statement?.predicate?.buildDefinition;
  const workflow = definition?.externalParameters?.workflow;
  const commit = definition?.resolvedDependencies?.find((item) => item.digest?.gitCommit)?.digest
    .gitCommit;
  const subject = statement?.subject?.find(
    (item) => item.name === `pkg:npm/%40ai-ecoverse/slicc-cdp@${plan.version}`
  );
  check(
    /^[a-f0-9]{64}$/.test(registry?.sha256 ?? '') && /^[a-f0-9]{128}$/.test(registry?.sha512 ?? ''),
    'registry byte digests unavailable',
    !!(registry?.sha256 && registry?.sha512)
  );
  const metadata = registry?.metadata;
  check(
    metadata?.name === name &&
      metadata?.version === plan.version &&
      registry?.manifest?.name === name &&
      registry?.manifest?.version === plan.version,
    'npm package identity mismatch',
    !!(metadata && registry?.manifest)
  );
  check(
    metadata?.dist?.integrity ===
      `sha512-${Buffer.from(registry?.sha512 ?? '', 'hex').toString('base64')}`,
    'npm integrity mismatch',
    !!metadata?.dist?.integrity
  );
  check(
    metadata?.gitHead === plan.commit &&
      registry?.tag?.type === 'commit' &&
      registry?.tag?.sha === plan.commit,
    'tag differs from npm gitHead',
    !!(metadata?.gitHead && registry?.tag)
  );
  check(
    statement?.predicateType === 'https://slsa.dev/provenance/v1',
    'SLSA v1 evidence absent or wrong',
    !!statement?.predicateType
  );
  check(
    subject?.digest?.sha512 === registry?.sha512,
    'SLSA subject mismatch',
    !!subject?.digest?.sha512
  );
  check(commit === plan.commit, 'SLSA gitCommit differs from gitHead/tag', !!commit);
  check(
    workflow?.repository === `https://github.com/${repo}` &&
      workflow?.ref === 'refs/heads/main' &&
      workflow?.path === '.github/workflows/release.yml',
    'SLSA workflow is not main release.yml',
    !!(workflow?.repository && workflow?.ref && workflow?.path)
  );
  const builder = statement?.predicate?.runDetails?.builder?.id;
  check(
    builder === 'https://github.com/actions/runner/github-hosted',
    'SLSA builder is not GitHub hosted',
    !!builder
  );
  check(
    ['identical', 'behind'].includes(registry?.ancestry),
    'tag is not on main',
    !!registry?.ancestry
  );
  const signatures = registry?.signatures;
  const completed = Number.isInteger(signatures?.exit) && !signatures.signal && !signatures.error;
  check(
    signatures?.exit === 0 && /verified attestation/.test(signatures?.stdout ?? ''),
    'npm signed attestation audit unavailable or failed',
    completed
  );
}
function verifyAssets() {
  const inventory = json(new URL('./inventory.json', import.meta.url));
  const linux = documents['assets-linux'];
  for (const file of [...inventory, 'SHA256SUMS']) {
    const row = linux?.files?.find((item) => item.file === file);
    check(
      row?.published === `sha256:${row?.sha256}`,
      `GitHub digest mismatch ${file}`,
      !!(row?.published && /^[a-f0-9]{64}$/.test(row?.sha256 ?? ''))
    );
  }
  for (const file of inventory) {
    const row = linux?.checksums?.files?.find((item) => item.file === file);
    check(
      row?.sha256 === row?.expected,
      row?.expected ? `checksum mismatch ${file}` : `${file} missing from SHA256SUMS`,
      !!(row?.sha256 && row?.expected)
    );
    const downloaded = linux?.files?.find((item) => item.file === file);
    check(
      row?.sha256 === downloaded?.sha256,
      `checksum/download mismatch ${file}`,
      !!(row?.sha256 && downloaded?.sha256)
    );
  }
  for (const file of inventory.filter((item) => item.includes('-darwin-'))) {
    const row = documents['assets-macos']?.files?.find((item) => item.file === file);
    const linuxRow = linux?.files?.find((item) => item.file === file);
    check(
      row?.sha256 === linuxRow?.sha256,
      `macOS/Linux checked different bytes ${file}`,
      !!(row?.sha256 && linuxRow?.sha256)
    );
    const signature = row?.signature;
    const steps = [signature?.display, signature?.strict, signature?.notarisation];
    const complete = steps.every(
      (step) => Number.isInteger(step?.exit) && !step.signal && !step.error
    );
    const identifier = file.startsWith('playwright-cli')
      ? 'ai.sliccy.playwright-cli'
      : 'ai.sliccy.curlwright';
    check(
      steps.every((step) => step?.exit === 0) &&
        signature?.display?.stderr?.includes(`Identifier=${identifier}\n`) &&
        signature.display.stderr.includes('Authority=Developer ID Application:') &&
        signature.display.stderr.includes('TeamIdentifier=S8LB56P782\n') &&
        signature?.notarisation?.stderr?.includes('source=Notarized Developer ID') &&
        /\baccepted\b/.test(signature?.notarisation?.stderr ?? ''),
      `signature identity/strict/notarisation ${file}`,
      complete
    );
  }
}
for (const validate of [verifyRegistry, verifyAssets]) {
  try {
    validate();
  } catch (error) {
    incomplete.push(`malformed evidence: ${error.message}`);
  }
}
const status = failures.length ? 'FAIL' : incomplete.length ? 'INCOMPLETE' : 'PASS';
const url = `https://github.com/${repo}/actions/runs/${plan.run}`;
const line =
  status === 'PASS'
    ? `certified ${name}@${plan.version} sha256=${registry.sha256} (supply-chain: registry=SLSA; release.yml@main; gitCommit=gitHead=tag=${plan.commit}; macOS=4 signed+notarised; checksums=complete; pr-certified: not asserted by this byte verification; ci: ${url})`
    : `cert-ci: ${status} ${name}@${plan.version}${status === 'FAIL' ? ' supply-chain alert' : ''} (${[...failures, ...incomplete].join('; ')})`;
const record = {
  ...identity(plan),
  schema: 1,
  status,
  certified: status === 'PASS',
  sha256: registry?.sha256,
  failures,
  incomplete,
  line,
  evidence: documents,
};
save(path.join(options.out, 'record.json'), record);
writeFileSync(path.join(options.out, 'line.txt'), `${line}\n`);
if (process.env.GITHUB_OUTPUT)
  appendFileSync(process.env.GITHUB_OUTPUT, `certified=${record.certified}\nstatus=${status}\n`);
if (process.env.GITHUB_STEP_SUMMARY)
  appendFileSync(
    process.env.GITHUB_STEP_SUMMARY,
    `# ${status === 'FAIL' ? 'SUPPLY-CHAIN FAIL' : `Supply-chain ${status}`}\n\n${line}\n`
  );
console.log(line);
