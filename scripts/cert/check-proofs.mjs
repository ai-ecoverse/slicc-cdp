import { parseArgs } from 'node:util';
import { json, save } from './common.mjs';

const { values } = parseArgs({
  options: {
    plan: { type: 'string' },
    positive: { type: 'string' },
    negative: { type: 'string' },
    source: { type: 'string', default: 'integration' },
    out: { type: 'string' },
  },
});
const plan = json(values.plan);
const positive = json(values.positive);
const negative = json(values.negative);
const tests = plan.proofs.tests.filter(
  (test) =>
    (values.source === 'guard'
      ? ['guard', 'guard-vector'].includes(test.source)
      : test.source === values.source) || test.missing
);
const rows = (execution) => [
  ...(execution.rows ?? []),
  ...(execution.report?.results ?? []).map((row) => ({
    proofId: `vector:${row.id}/${row.target}/${row.variant}`,
    type: [row.control, row.guarded, row.live].includes('fail') ? 'test:fail' : 'test:pass',
    name: row.id,
    failureType: 'testCodeFailure',
    skip: row.control === 'skip' || row.control === 'not-run',
  })),
];
const currentRows = rows(positive);
const previousRows = rows(negative);
const incomplete = [];
const failures = [];
const expected = tests.map((test) => test.id).sort();
const behavioural = (row) =>
  row.name &&
  row.name !== row.file &&
  ['test:pass', 'test:fail'].includes(row.type) &&
  row.failureType !== 'subtestsFailed';
const meaningful = (row) =>
  row.type === 'test:fail' &&
  row.failureType === 'testCodeFailure' &&
  !/running 0 tests|could not compile|ERR_MODULE_NOT_FOUND|Cannot find module/.test(
    row.error ?? ''
  );
const match = (row, test) =>
  row.proofId === test.id || (row.file?.endsWith(test.file) && row.name === test.title);
const results = tests.map((test) => {
  const current = test.missing
    ? []
    : currentRows.filter((row) => match(row, test) && behavioural(row));
  const previous = test.missing
    ? []
    : previousRows.filter((row) => match(row, test) && behavioural(row));
  if (
    !current.length ||
    !previous.length ||
    current.some((row) => row.skip || row.todo) ||
    previous.some((row) => row.skip || row.todo)
  )
    incomplete.push(`named proof ${test.id} did not run on both versions`);
  else if (!current.some((row) => row.type === 'test:pass'))
    failures.push(`named proof ${test.id} fails on the candidate`);
  else if (!previous.some(meaningful)) incomplete.push(`proof does not discriminate: ${test.id}`);
  return { id: test.id, current, previous };
});
if (expected.length && (!positive.attempts?.length || !negative.attempts?.length))
  incomplete.push('named proof NEGATIVE did not run');
if (
  expected.length &&
  (negative.status === 'INCOMPLETE' ||
    negative.missing?.length ||
    (negative.rows ?? []).some(
      (row) =>
        row.skip ||
        row.todo ||
        ['cancelledByParent', 'testTimeoutFailure'].includes(row.failureType)
    ))
)
  incomplete.push('named proof NEGATIVE worker did not complete');
const observed = [
  ...new Set(
    previousRows
      .filter((row) => behavioural(row) && row.type === 'test:fail')
      .map((row) => {
        const test = tests.find((test) => match(row, test));
        return test?.id ?? row.proofId ?? `${row.worker ?? row.file}#${row.name}`;
      })
  ),
].sort();
const unexpected = observed.filter((id) => !expected.includes(id));
const absent = expected.filter((id) => !observed.includes(id));
for (const id of absent)
  if (!incomplete.some((reason) => reason.includes(id)))
    incomplete.push(`proof does not discriminate: ${id}`);
const status = failures.length ? 'FAIL' : incomplete.length ? 'INCOMPLETE' : 'PASS';
const result = { status, expected, observed, unexpected, incomplete, failures, results };
save(values.out, result);
console.log(JSON.stringify(result));
