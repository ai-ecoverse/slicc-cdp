import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { cliVerdict } from './cli-verdict.mjs';
import { args, digest, identity, json, root, run, save } from './common.mjs';
import { artifactFindings, artifactInventory } from './pr-artifact-binding.mjs';
import { selectBiosPin } from './pr-bios-result.mjs';
import { client, required, residual } from './pr-common.mjs';
import { unpackLocal } from './pr-subject.mjs';
import { validateBios, validateGuard, validateMac, validateSuite } from './pr-validate.mjs';

const printVerdict = cliVerdict('pr-record: ');
const options = args({ evidence: { type: 'string' }, bindings: { type: 'string' } });
const plan = json(path.join(options.evidence, 'plan.json'));
const failures = [];
const incomplete = [...(plan.issues ?? [])];
const flags = [];
const emit = (status, reason) => (status === 'FAIL' ? failures : incomplete).push(reason);
let artifactBinding;
const bindingContext = {};
const bound = options.bindings !== undefined;
if (bound) {
  try {
    const { api } = client(plan.repository);
    const inventory = await artifactInventory(api, plan.run);
    let bindings;
    try {
      bindings = json(options.bindings);
    } catch (error) {
      emit('INCOMPLETE', `artifacts: ${error.message}`);
    }
    bindingContext.bindings = bindings;
    bindingContext.inventory = inventory;
    artifactBinding = artifactFindings(plan, bindings, inventory, { phase: 'record' });
    for (const reason of artifactBinding.failures) emit('FAIL', `artifacts: ${reason}`);
    for (const reason of artifactBinding.incomplete) emit('INCOMPLETE', `artifacts: ${reason}`);
  } catch (error) {
    emit('INCOMPLETE', `artifacts: ${error.message}`);
  }
}
if (
  plan.mode !== 'pr' ||
  !plan.go ||
  !/^[a-f0-9]{40}$/.test(plan.head ?? '') ||
  !/^[a-f0-9]{40}$/.test(plan.commit ?? '')
)
  throw new Error('invalid eligible PR plan');
const documents = {};
for (const which of required) {
  try {
    const item = json(path.join(options.evidence, `${which}.json`));
    if (
      JSON.stringify(identity(item)) !== JSON.stringify(identity(plan)) ||
      item.which !== which ||
      !['PASS', 'FAIL', 'INCOMPLETE'].includes(item.status)
    )
      throw new Error('identity or result mismatch');
    documents[which] = item;
    if (item.status !== 'PASS') emit(item.status, `${which}: ${item.reason ?? item.status}`);
  } catch (error) {
    emit('INCOMPLETE', `${which}: ${error.message}`);
  }
}
let candidate;
let baseline;
try {
  candidate = unpackLocal(plan, plan.version, path.join(root, '.cert/record-candidate'));
  baseline = unpackLocal(plan, plan.baseline.version, path.join(root, '.cert/record-base'));
} catch (error) {
  emit(error.certStatus ?? 'INCOMPLETE', error.message);
}
for (const which of ['rebuild', 'guard', 'integration']) {
  const item = documents[which];
  if (item?.sha256 && candidate?.sha256 && item.sha256 !== candidate.sha256)
    emit('FAIL', `${which}: checked another pack`);
}
for (const [leg, subject, checked] of [
  ['positive', { commit: plan.commit, version: plan.version }, candidate],
  ['negative', plan.baseline, baseline],
]) {
  const execution = documents.guard?.executions?.[leg];
  if (checked && execution?.sha256 && execution.sha256 !== checked.sha256)
    emit('FAIL', `guard ${leg}: checked another pack`);
  const integrationSubject = documents.integration?.executions?.[leg]?.subject;
  if (
    !integrationSubject ||
    integrationSubject.commit !== subject.commit ||
    integrationSubject.version !== subject.version ||
    integrationSubject.sha256 !== checked?.sha256
  )
    emit('INCOMPLETE', `integration ${leg}: pack/source identity missing`);
}
const rebuild = documents.rebuild;
if (
  !Array.isArray(rebuild?.files) ||
  rebuild.files.length !== 2 ||
  !['bin/playwright-cli.wasm', 'bin/curlwright.wasm'].every((file) =>
    rebuild.files.some(
      (row) =>
        row?.file === file &&
        /^[a-f0-9]{64}$/.test(row.built ?? '') &&
        /^[a-f0-9]{64}$/.test(row.published ?? '')
    )
  )
)
  emit('INCOMPLETE', 'rebuild: both wasm comparisons required');
else if (rebuild.files.some((row) => row.built !== row.published))
  emit('FAIL', 'rebuild: wasm byte mismatch');
const vectors = JSON.parse(
  readFileSync(
    new URL(import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')),
    'utf8'
  )
);
if (vectors.setVersion !== plan.vectors)
  emit('INCOMPLETE', 'record guard runner differs from plan');
validateGuard(documents.guard, plan, vectors, emit);
validateSuite(documents.integration?.executions?.positive, 'integration positive', emit);
if (plan.changedTests?.length)
  validateSuite(documents.integration?.executions?.negative, 'changed-test NEGATIVE', emit, false);
for (const [source, positive, negative] of [
  [
    'integration',
    documents.integration?.executions?.proofs,
    documents.integration?.executions?.negative,
  ],
  ['guard', documents.guard?.executions?.positive, documents.guard?.executions?.negative],
]) {
  const directory = path.join(options.out, `proof-${source}`);
  save(path.join(directory, 'positive.json'), positive ?? {});
  save(path.join(directory, 'negative.json'), negative ?? {});
  try {
    run(process.execPath, [
      path.join(root, 'scripts/cert/check-proofs.mjs'),
      '--plan',
      path.join(options.evidence, 'plan.json'),
      '--source',
      source,
      '--positive',
      path.join(directory, 'positive.json'),
      '--negative',
      path.join(directory, 'negative.json'),
      '--out',
      path.join(directory, 'result.json'),
    ]);
    const proofs = json(path.join(directory, 'result.json'));
    failures.push(...proofs.failures);
    incomplete.push(...proofs.incomplete);
    if (proofs.unexpected.length)
      flags.push(`unexpected NEGATIVE: ${proofs.unexpected.join(', ')}`);
  } catch (error) {
    emit('INCOMPLETE', `named ${source}: ${error.message}`);
  }
}
const biosPin = selectBiosPin(json(new URL('./pr-bios.json', import.meta.url)), bound);
flags.push(
  ...validateBios(
    documents.bios,
    biosPin,
    emit,
    plan,
    { base: baseline, candidate },
    bindingContext
  )
);
try {
  const checked = path.join(options.out, 'checksums.json');
  try {
    run(process.execPath, [
      path.join(root, 'scripts/cert/asset-check.mjs'),
      '--directory',
      process.env.CERT_PR_ASSETS,
      '--mode',
      'pr',
      '--out',
      checked,
    ]);
  } catch {}
  const checksums = json(checked);
  if (checksums.status !== 'PASS')
    emit(checksums.status, 'assets: independent strict checksum inventory did not pass');
  const manifest = json(path.join(process.env.CERT_PR_ASSETS, 'asset-manifest.json'));
  if (
    JSON.stringify(identity(manifest)) !== JSON.stringify(identity(plan)) ||
    manifest.sources?.length !== 6 ||
    manifest.signed?.files?.length !== 4
  )
    emit('INCOMPLETE', 'assets: source/signing inventory incomplete');
  const platforms = json(new URL('./pr-native.json', import.meta.url));
  for (const platform of platforms) {
    const item = manifest.sources?.find(
      (source) => source.label === platform.label && source.target === platform.target
    );
    if (
      !item ||
      JSON.stringify(identity(item)) !== JSON.stringify(identity(plan)) ||
      item.side !== 'candidate' ||
      item.subject?.commit !== plan.commit ||
      item.subject?.version !== plan.version ||
      item.lockSha256 !== plan.lockSha256
    ) {
      emit('INCOMPLETE', `assets: native source/lock identity mismatch ${platform.label}`);
      continue;
    }
    for (const row of item.files ?? []) {
      const signed = row.file.includes('-darwin-')
        ? manifest.signed.files.find((entry) => entry.file === row.file)
        : null;
      if (signed && signed.input !== row.sha256)
        emit('FAIL', `assets: signer input mismatch ${row.file}`);
      const expected = signed ? signed.sha256 : row.sha256;
      if (digest(path.join(process.env.CERT_PR_ASSETS, row.file)) !== expected)
        emit('FAIL', `assets: source/signer bytes mismatch ${row.file}`);
    }
  }
  for (const row of documents['assets-macos']?.files ?? [])
    if (row.sha256 !== digest(path.join(process.env.CERT_PR_ASSETS, row.file)))
      emit('FAIL', `macos: verified another binary ${row.file}`);
} catch (error) {
  emit('INCOMPLETE', `assets: ${error.message}`);
}
validateMac(documents['assets-macos'], json(new URL('./inventory.json', import.meta.url)), emit);
const review = documents.review;
if (
  review?.head !== plan.head ||
  review?.commit !== plan.commit ||
  !review?.execution ||
  !review?.verdict?.reason ||
  review.verdict.status !== review.status
)
  emit('INCOMPLETE', 'review: bound verdict and execution receipt required');
const upstream = JSON.parse(process.env.CERT_UPSTREAM ?? '{}');
for (const job of [
  'pack',
  'native',
  'signing',
  'collect',
  'rebuild',
  'guard',
  'integration',
  'bios',
  'assets',
  'review',
])
  if (upstream[job] !== 'success')
    emit('INCOMPLETE', `${job}: upstream ${upstream[job] ?? 'missing'}`);
if (
  plan.self?.status !== 'PASS' ||
  !plan.self.members?.some(
    (member) => member.pr === plan.pr && member.head === plan.head && member.status === 'PASS'
  )
)
  emit('INCOMPLETE', 'shared self-guard receipt required');
const status = failures.length ? 'FAIL' : incomplete.length ? 'INCOMPLETE' : 'PASS';
const known = [...(plan.proofs?.known ?? [])];
const line =
  status === 'PASS'
    ? `certified PR #${plan.pr} head=${plan.head} merge=${plan.commit} sha256=${candidate?.sha256} guard=GUARD-PASS bios=${biosPin.commit}`
    : `cert-pr: ${status} #${plan.pr} head=${plan.head} (${[...failures, ...incomplete].join('; ')})`;
const record = {
  ...identity(plan),
  schema: 1,
  status,
  certified: status === 'PASS',
  behaviour: true,
  sha256: candidate?.sha256,
  baselineSha256: baseline?.sha256,
  line,
  failures,
  incomplete,
  flags,
  known,
  testOnly: plan.testOnly,
  fullReviewTriggered: true,
  bios: biosPin,
  required,
  self: plan.self,
  upstream,
  ...(bound ? { artifactBinding } : {}),
  residual,
};
save(path.join(options.out, 'record.json'), record);
writeFileSync(path.join(options.out, 'line.txt'), `${line}\n`);
console.log(line);
printVerdict(record.status, options.out);
