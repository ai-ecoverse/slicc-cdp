import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import test from 'node:test';
import yaml from 'js-yaml';

function loadWorkflow() {
  const file = new URL('../../.github/workflows/cert-review-pr.yml', import.meta.url);
  assert.ok(existsSync(file), 'PR certification workflow must exist');
  return yaml.load(readFileSync(file, 'utf8'));
}
const steps = (job) => loadWorkflow().jobs[job].steps;

test('PR certification workflow reports despite skipped evidence ancestors', () => {
  const workflow = loadWorkflow();
  assert.deepEqual(workflow.on.pull_request.types, [
    'opened',
    'synchronize',
    'reopened',
    'ready_for_review',
    'converted_to_draft',
    'labeled',
    'unlabeled',
  ]);
  assert.equal(
    workflow.concurrency.group,
    "${{ ((github.event.action == 'labeled' || github.event.action == 'unlabeled') && github.event.label.name != 'cert-review-approved') && format('cert-pr-noop-{0}', github.run_id) || format('cert-pr-{0}', github.event.pull_request.number || github.event.merge_group.head_sha) }}"
  );
  assert.equal(
    workflow.concurrency['cancel-in-progress'],
    "${{ github.event_name == 'pull_request' }}"
  );
  assert.doesNotMatch(
    readFileSync(new URL('../../.github/workflows/cert-review-pr.yml', import.meta.url), 'utf8'),
    /merge-ok/
  );
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
    'bios_inputs',
    'bios_base',
    'bios_candidate',
    'assets',
    'review',
    'record',
  ]) {
    assert.ok(workflow.jobs[name], `required certification job ${name} exists`);
    assert.match(workflow.jobs[name].if, /github\.event_name == 'pull_request'/);
  }
  const group = steps('report').find((step) => step.run?.includes('--action group'));
  assert.match(group.if, /always\(\).*github\.event_name == 'merge_group'/);
  assert.match(group.run, /scripts\/cert\/pr-report\.mjs/);
});

test('PR certification controls come from main and candidate jobs stay hosted read-only', () => {
  const workflow = loadWorkflow();
  assert.equal(workflow.env.SLICC_CDP_PORTS, '9300-9309');
  assert.deepEqual(workflow.permissions, { contents: 'read' });
  for (const job of Object.values(workflow.jobs)) {
    if (!job.steps) {
      assert.equal(
        job.uses,
        'ai-ecoverse/slicc-bios/.github/workflows/canary.yml@4025d6aabc2c06add216183064d81914a171cd64'
      );
      assert.deepEqual(job.permissions, { contents: 'read', actions: 'read' });
      continue;
    }
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
  const workflow = loadWorkflow();
  assert.ok(!JSON.stringify(workflow.env).includes('secrets.'));
  for (const job of Object.values(workflow.jobs))
    assert.ok(!JSON.stringify(job.env ?? {}).includes('secrets.'));
  const secretSteps = Object.entries(workflow.jobs).flatMap(([name, job]) =>
    (job.steps ?? [])
      .filter((step) => JSON.stringify(step.env ?? {}).includes('secrets.'))
      .map(() => name)
  );
  assert.deepEqual(secretSteps, ['signing', 'review']);
  const model = steps('review').find((step) => step.id === 'model');
  assert.equal(model.env.NODE_VERSION, '24');
  assert.equal(
    model.uses,
    'anthropics/claude-code-action/base-action@a26b07d8c778b0c43f64618e5fd29fd0f8af72e8'
  );
  assert.deepEqual(
    model.with.claude_args
      .split('\n')
      .map((line) => line.trim())
      .filter((line) => /^--(?:tools|allowed-?tools|disallowed-?tools|mcp)/i.test(line)),
    ['--tools=']
  );
  assert.ok(!model.with.claude_args.includes('--json-schema'));
  assert.ok(!JSON.stringify(workflow.jobs.review).includes('structured_output'));
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
  assert.ok(receipt.run.includes('--execution "${EXECUTION:-/dev/null}"'));
  assert.ok(!receipt.run.includes('--verdict'));
  const prepare = steps('review').find((step) => step.run?.includes('EXACT SOURCE DIFF DATA'));
  assert.match(prepare.run, /--no-ext-diff.*--no-textconv/);
  assert.ok(!prepare.run.includes('npm ci'));
});

test('PR certification preserves canonical plan and missing producer refusal', () => {
  const workflow = loadWorkflow();
  const calls = Object.values(workflow.jobs).flatMap((job) =>
    (job.steps ?? []).flatMap((step) =>
      (step.run ?? '').split('\n').filter((line) => /node scripts\/cert\/[\w-]+\.mjs/.test(line))
    )
  );
  assert.equal(calls.length, 19);
  for (const call of calls) {
    const gated = call.match(
      /node scripts\/gate\.mjs "?([\w$-]+)"? --accept (PASS(?:,FAIL,INCOMPLETE)?) -- node scripts\/cert\/([\w-]+)\.mjs/
    );
    assert.ok(gated, call);
    assert.equal(gated[1], gated[3] === 'assets' ? 'assets-$PLATFORM' : gated[3]);
    assert.equal(gated[2], gated[3] === 'pr-bios-files' ? 'PASS' : 'PASS,FAIL,INCOMPLETE');
  }
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
  assert.ok(bios.needs.includes('bios_base') && bios.needs.includes('bios_candidate'));
  assert.ok(!bios.if.includes('needs.bios_base.result'));
  for (const side of ['base', 'candidate']) {
    const call = workflow.jobs[`bios_${side}`];
    assert.equal(
      call.uses,
      'ai-ecoverse/slicc-bios/.github/workflows/canary.yml@4025d6aabc2c06add216183064d81914a171cd64'
    );
    assert.equal(call.with.label, side);
    assert.equal(call.with.repeat, 3);
    assert.equal(call.with['bios-ref'], '4025d6aabc2c06add216183064d81914a171cd64');
    assert.deepEqual(JSON.parse(call.with.shards), ['packages-update-cdp', 'guard']);
  }
  const consume = bios.steps.find((step) => step.run?.includes('scripts/cert/pr-bios-files.mjs'));
  assert.ok(consume, 'main-owned file writer must replace whole-result environment transport');
  assert.equal(consume.env, undefined);
  assert.ok(consume.run.includes("<<'EOF'"));
  for (const side of ['base', 'candidate']) {
    assert.ok(consume.run.includes(`toJSON(needs.bios_${side}.outputs.result || 'null')`));
    assert.ok(consume.run.includes(`--${side} evidence/bios-${side}.json`));
  }
  assert.ok(!JSON.stringify(bios).includes('BIOS_BASE'));
  assert.ok(!JSON.stringify(bios).includes('BIOS_CANDIDATE'));
  assert.ok(!consume.run.includes('fromJSON'));
  assert.ok(consume.run.includes('pr-bios-files --accept PASS --'));
  assert.ok(consume.run.indexOf('pr-bios-files --') < consume.run.indexOf('pr-bios --'));
  for (const name of ['guard', 'integration']) {
    const raw = steps(name).find((step) => step.uses?.startsWith('actions/upload-artifact@'));
    assert.ok(
      raw.with.path
        .split('\n')
        .filter(Boolean)
        .every((file) => /\.(jsonl?|log|md)$/.test(file))
    );
  }
  for (const name of ['self', 'report']) {
    const guard = steps(name).find((step) => step.id === 'guard');
    for (const file of [
      '.gitattributes',
      'scripts/test-integration.mjs',
      'test/integration/test-arguments.mjs',
      'test/integration/ports.mjs',
    ])
      assert.ok(guard.with.paths.split('\n').includes(file));
  }
});

test('Non-required PR certification jobs cannot alias required CI check names', () => {
  const workflow = loadWorkflow();
  const required = ['integration', 'ci / test', 'shellcheck'];
  for (const [id, job] of Object.entries(workflow.jobs)) {
    assert.ok(!required.includes(job.name ?? id));
    if (id !== 'report') assert.ok(job.name.startsWith('cert-review-pr / '));
  }
  assert.equal(workflow.jobs.report.name, 'cert-review');
});

test('PR certification signed bytes expire after one day while verification metadata remains', () => {
  const workflow = loadWorkflow();
  for (const [name, job] of Object.entries(workflow.jobs)) {
    for (const step of (job.steps ?? []).filter((item) =>
      item.uses?.startsWith('actions/upload-artifact@')
    )) {
      const signedBytes = name === 'signing' || name === 'collect';
      assert.equal(step.with['retention-days'], signedBytes ? 1 : 90);
    }
  }
});
