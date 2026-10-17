import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import yaml from 'js-yaml';

const workflow = yaml.load(
  readFileSync(new URL('../../.github/workflows/cert-review-pr.yml', import.meta.url), 'utf8')
);
const steps = (job) => workflow.jobs[job].steps;

test('PR certification workflow reports despite skipped evidence ancestors', () => {
  assert.deepEqual(workflow.on.pull_request.types, [
    'opened',
    'synchronize',
    'reopened',
    'labeled',
    'unlabeled',
    'ready_for_review',
  ]);
  assert.equal(workflow.on.pull_request_target, undefined);
  assert.deepEqual(workflow.on.merge_group.types, ['checks_requested']);
  assert.match(workflow.jobs.report.if, /always\(\)/);
  assert.match(workflow.jobs.record.if, /always\(\)/);
  assert.ok(workflow.jobs.report.needs.includes('record'));
  assert.ok(workflow.jobs.report.needs.includes('pack'));
  for (const name of [
    'plan',
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
    'record',
  ]) {
    assert.match(workflow.jobs[name].if, /github\.event_name == 'pull_request'/);
  }
  const group = steps('report').find((step) => step.run?.includes('--action group'));
  assert.match(group.if, /always\(\).*github\.event_name == 'merge_group'/);
  assert.match(group.run, /scripts\/cert\/pr-report\.mjs/);
});

test('PR certification controls come from main and candidate jobs stay hosted read-only', () => {
  assert.equal(workflow.env.SLICC_CDP_PORTS, '9300-9309');
  assert.deepEqual(workflow.permissions, { contents: 'read' });
  for (const job of Object.values(workflow.jobs)) {
    const checkouts = job.steps.filter((step) => step.uses?.startsWith('actions/checkout@'));
    assert.equal(checkouts.length, 1);
    assert.equal(checkouts[0].with['persist-credentials'], false);
    assert.equal(checkouts[0].with['fetch-depth'], 0);
    assert.equal(
      checkouts[0].with.ref,
      '${{ github.event.pull_request.base.sha || github.event.merge_group.base_sha }}'
    );
  }
  for (const name of [
    'pack',
    'native',
    'collect',
    'rebuild',
    'guard',
    'integration',
    'bios',
    'assets',
  ]) {
    const job = workflow.jobs[name];
    assert.deepEqual(job.permissions, { contents: 'read' });
    const runners = job.strategy?.matrix?.include?.map((row) => row.os) ??
      job.strategy?.matrix?.os ?? [job['runs-on']];
    assert.ok(
      runners.every((runner) =>
        ['ubuntu-latest', 'macos-latest', 'windows-latest'].includes(runner)
      )
    );
    assert.ok(!JSON.stringify(job).includes('secrets.'));
  }
});

test('PR certification credentials are step-only and model has no execution or filesystem tools', () => {
  assert.ok(!JSON.stringify(workflow.env).includes('secrets.'));
  for (const job of Object.values(workflow.jobs))
    assert.ok(!JSON.stringify(job.env ?? {}).includes('secrets.'));
  const secretSteps = Object.entries(workflow.jobs).flatMap(([name, job]) =>
    job.steps.filter((step) => JSON.stringify(step.env ?? {}).includes('secrets.')).map(() => name)
  );
  assert.deepEqual(secretSteps, ['signing', 'review']);
  const model = steps('review').find((step) => step.id === 'model');
  assert.equal(model.env.NODE_VERSION, '24');
  assert.match(model.with.claude_args, /--tools ""/);
  assert.match(model.with.claude_args, /--json-schema/);
  assert.equal(model.with.allowed_bots, undefined);
  assert.equal(model.with.prompt_file, 'review/prompt.txt');
  assert.ok(
    model.with.claude_args.includes("vars.CERT_BEDROCK_MODEL || 'global.anthropic.claude-opus-5-5'")
  );
  const receipt = steps('review').find((step) => step.env?.CERT_REVIEW_MODEL);
  assert.equal(
    receipt.env.CERT_REVIEW_MODEL,
    "${{ vars.CERT_BEDROCK_MODEL || 'global.anthropic.claude-opus-5-5' }}"
  );
  assert.ok(receipt.run.includes('value.model=process.env.CERT_REVIEW_MODEL'));
  const prepare = steps('review').find((step) => step.run?.includes('EXACT SOURCE DIFF DATA'));
  assert.match(prepare.run, /--no-ext-diff.*--no-textconv/);
  assert.ok(!prepare.run.includes('npm ci'));
});

test('PR certification preserves canonical plan and missing producer refusal', () => {
  const gather = steps('record').find((step) => step.name?.startsWith('Gather only canonical'));
  assert.match(
    gather.run,
    /for name in rebuild guard integration bios assets-linux assets-macos review/
  );
  assert.ok(!gather.run.includes('find '));
  assert.ok(!gather.run.includes('plan.json'));
  const input = steps('report').find((step) =>
    step.with?.name?.startsWith('cert-pr-produced-record-')
  );
  assert.equal(input['continue-on-error'], true);
  const publish = steps('report').find((step) =>
    step.run?.includes('--record produced-record/record.json')
  );
  assert.match(publish.if, /always\(\)/);
  const archive = steps('report').find((step) => step.uses?.startsWith('actions/upload-artifact@'));
  assert.equal(archive.with.path, 'reported/record.json');
  assert.ok(archive.with.name.includes('github.event.pull_request.head.sha'));
  assert.ok(archive.with.name.includes('github.run_attempt'));
  assert.equal(archive.with.overwrite, undefined);
  const bios = workflow.jobs.bios;
  assert.ok(bios.steps.some((step) => step.run?.includes('scripts/cert/pr-bios.mjs')));
  assert.ok(!JSON.stringify(bios).includes('slicc-bios/'));
});

test('PR certification signed bytes expire after one day while verification metadata remains', () => {
  for (const [name, job] of Object.entries(workflow.jobs)) {
    for (const step of job.steps.filter((item) =>
      item.uses?.startsWith('actions/upload-artifact@')
    )) {
      const signedBytes = name === 'signing' || name === 'collect';
      assert.equal(step.with['retention-days'], signedBytes ? 1 : 90);
    }
  }
});
