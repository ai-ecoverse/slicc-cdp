import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { cliVerdict } from './cli-verdict.mjs';
import { identity, json, root, run, save } from './common.mjs';
import { client, context, hash, required, residual } from './pr-common.mjs';

const printVerdict = cliVerdict('pr-report: ');
const { values } = parseArgs({
  options: {
    action: { type: 'string', default: 'pr' },
    event: { type: 'string', default: process.env.GITHUB_EVENT_PATH },
    plan: { type: 'string' },
    record: { type: 'string' },
    self: { type: 'string' },
    out: { type: 'string' },
  },
});
const event = json(values.event);
const repository = event.repository.full_name;
const { api, pages } = client(repository);
const runUrl = `https://github.com/${repository}/actions/runs/${process.env.GITHUB_RUN_ID}`;
const marker = '<!-- cert-review-pr -->';

async function publish(head, record) {
  if (!/^[a-f0-9]{40}$/.test(head)) throw new Error('status head invalid');
  await api(`statuses/${head}`, 'POST', {
    context,
    state: record.status === 'PASS' ? 'success' : record.status === 'FAIL' ? 'failure' : 'error',
    description: record.line.slice(0, 140),
    target_url: runUrl,
  });
}

async function reviewedRecord(plan, current) {
  let record;
  try {
    record = json(values.record);
  } catch {
    record = {
      ...identity(plan),
      schema: 1,
      status: 'INCOMPLETE',
      failures: [],
      incomplete: ['required record producer absent'],
    };
  }
  if (JSON.stringify(identity(record)) !== JSON.stringify(identity(plan)))
    throw new Error('record source/run identity mismatch');
  if (
    record.status === 'PASS' &&
    (record.schema !== 1 ||
      record.certified !== true ||
      JSON.stringify(record.required) !== JSON.stringify(required) ||
      !/^[a-f0-9]{64}$/.test(record.sha256 ?? '') ||
      record.failures?.length ||
      record.incomplete?.length)
  )
    record = {
      ...record,
      status: 'INCOMPLETE',
      incomplete: ['complete code-generated certificate required'],
    };
  if (current.draft !== false)
    record = {
      ...record,
      status: 'INCOMPLETE',
      incomplete: ['converted to draft: not proof'],
    };
  if (plan.reviewer) {
    const latest = (await pages(`issues/${plan.pr}/timeline`))
      .filter(
        (entry) =>
          ['labeled', 'unlabeled'].includes(entry.event) &&
          entry.label?.name === 'cert-review-approved'
      )
      .at(-1);
    const approval = plan.self?.members?.find(
      (member) => member.pr === plan.pr && member.head === plan.head
    );
    if (
      !current.labels.some((label) => label.name === 'cert-review-approved') ||
      latest?.event !== 'labeled' ||
      latest.id !== approval?.event
    )
      record = {
        ...record,
        status: 'INCOMPLETE',
        incomplete: ['coordinator approval event changed or removed'],
      };
  }
  return record;
}

async function pr() {
  const plan = json(values.plan);
  if (!plan.report) return { status: 'NOOP', reason: plan.reason, head: plan.head };
  const current = await api(`pulls/${plan.pr}`);
  if (current.head.sha !== plan.head || current.state !== 'open')
    return { status: 'NOOP', reason: 'stale head or closed PR', head: plan.head };
  let record;
  if (plan.go) {
    try {
      record = await reviewedRecord(plan, current);
    } catch (error) {
      record = {
        ...identity(plan),
        schema: 1,
        status: 'INCOMPLETE',
        certified: false,
        failures: [],
        incomplete: [error.message],
      };
    }
  } else {
    record = {
      ...identity(plan),
      schema: 1,
      status: plan.status,
      certified: false,
      behaviour: plan.behaviour,
      reason: plan.reason,
      required: [],
      self: plan.self,
      testOnly: plan.testOnly,
      fullReviewTriggered: false,
      residual,
    };
  }
  record.line =
    record.status === 'PASS'
      ? plan.go
        ? record.line
        : `cert-review-pr: not behaviour #${plan.pr} head=${plan.head}`
      : `cert-review-pr: ${record.status} #${plan.pr} head=${plan.head} (${record.reason ?? [...(record.failures ?? []), ...(record.incomplete ?? [])].join('; ')})`;
  const latest = await api(`pulls/${plan.pr}`);
  if (latest.head.sha !== plan.head || latest.state !== 'open')
    return { status: 'NOOP', reason: 'stale head or closed PR before reporting', head: plan.head };
  const comments = await pages(`issues/${plan.pr}/comments`);
  const sticky = comments.find(
    (comment) => comment.user?.login === 'github-actions[bot]' && comment.body.startsWith(marker)
  );
  let previous = {};
  try {
    previous = JSON.parse(sticky?.body.split('\n')[1] ?? '{}');
  } catch {}
  record.testOnlyReviewEver = previous.testOnlyReviewEver === true || (plan.testOnly && plan.go);
  record.testOnlyPRCountContribution = record.testOnlyReviewEver ? 1 : 0;
  const body = `${marker}\n${JSON.stringify({ pr: plan.pr, head: plan.head, testOnlyReviewEver: record.testOnlyReviewEver })}\n\n${record.line}\n\n[Raw evidence and record](${runUrl})\n\nTest-only PR full-review count contribution: ${record.testOnlyPRCountContribution} (deduplicate PR #${plan.pr} across records).\n\n${residual}`;
  if (sticky) await api(`issues/comments/${sticky.id}`, 'PATCH', { body });
  else await api(`issues/${plan.pr}/comments`, 'POST', { body });
  await publish(plan.head, record);
  return record;
}

async function certified(number, head) {
  const statuses = await pages(`commits/${head}/statuses`);
  const status = statuses.find((item) => item.context === context);
  if (status?.state !== 'success' || status.creator?.login !== 'github-actions[bot]')
    throw new Error(`#${number}: trusted successful status absent`);
  const escaped = repository.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = status.target_url?.match(
    new RegExp(`^https://github\\.com/${escaped}/actions/runs/(\\d+)$`)
  );
  if (!match) throw new Error(`#${number}: status does not link its workflow run`);
  const workflowRun = await api(`actions/runs/${match[1]}`);
  if (
    workflowRun.path !== '.github/workflows/cert-review-pr.yml' ||
    workflowRun.event !== 'pull_request' ||
    workflowRun.head_sha !== head ||
    workflowRun.repository.full_name !== repository ||
    workflowRun.status !== 'completed' ||
    workflowRun.conclusion !== 'success'
  )
    throw new Error(`#${number}: cert run provenance or outcome invalid`);
  const name = `cert-pr-record-${number}-${head}-${workflowRun.run_attempt}`;
  const result = await api(`actions/runs/${match[1]}/artifacts?per_page=100`);
  if (result.total_count > 100) throw new Error('artifact listing truncated');
  const archives = result.artifacts.filter(
    (artifact) => artifact.name === name && !artifact.expired
  );
  if (archives.length !== 1 || !/^sha256:[a-f0-9]{64}$/.test(archives[0].digest ?? ''))
    throw new Error(`#${number}: immutable record artifact absent`);
  const response = await fetch(
    `${process.env.GITHUB_API_URL ?? 'https://api.github.com'}/repos/${repository}/actions/artifacts/${archives[0].id}/zip`,
    {
      headers: {
        ...(process.env.GH_TOKEN ? { Authorization: `Bearer ${process.env.GH_TOKEN}` } : {}),
      },
      signal: AbortSignal.timeout(60000),
    }
  );
  if (!response.ok) throw new Error(`record archive HTTP ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  if (`sha256:${hash(bytes)}` !== archives[0].digest)
    throw Object.assign(new Error('record archive digest mismatch'), { certStatus: 'FAIL' });
  const directory = path.join(root, '.cert/group', match[1]);
  mkdirSync(directory, { recursive: true });
  const zip = path.join(directory, 'record.zip');
  writeFileSync(zip, bytes);
  const entries = run('unzip', ['-Z1', zip]).split('\n');
  if (entries.filter((entry) => entry === 'record.json').length !== 1)
    throw new Error('canonical record entry missing');
  const decoded = execFileSync('unzip', ['-p', zip, 'record.json'], { maxBuffer: 8 * 1024 * 1024 });
  const record = JSON.parse(decoded);
  if (
    record.schema !== 1 ||
    record.status !== 'PASS' ||
    record.repository !== repository ||
    record.pr !== number ||
    record.head !== head ||
    record.run !== String(workflowRun.id) ||
    record.attempt !== String(workflowRun.run_attempt) ||
    record.self?.status !== 'PASS'
  )
    throw new Error(`#${number}: exact-head record invalid`);
  const notBehaviour =
    record.behaviour === false &&
    record.reason === 'not behaviour' &&
    record.fullReviewTriggered === false;
  if (
    !notBehaviour &&
    (!record.certified ||
      !record.fullReviewTriggered ||
      !/^[a-f0-9]{64}$/.test(record.sha256 ?? '') ||
      JSON.stringify(record.required) !== JSON.stringify(required) ||
      record.failures?.length ||
      record.incomplete?.length)
  )
    throw new Error(`#${number}: incomplete behavioural certificate`);
  return { ...record, sourceRun: workflowRun.id, artifact: archives[0].id };
}

async function group() {
  const { base_sha: base, head_sha: head } = event.merge_group;
  if (![base, head].every((sha) => /^[a-f0-9]{40}$/.test(sha)))
    throw new Error('group SHA invalid');
  const self = values.self ? json(values.self) : null;
  if (
    self?.status !== 'PASS' ||
    self.repository !== repository ||
    self.head !== head ||
    self.base !== base
  )
    throw new Error('fresh exact-group self-guard required');
  run('git', ['merge-base', '--is-ancestor', base, head]);
  const records = [];
  const failures = [];
  const incomplete = [];
  let commit = head;
  while (commit !== base) {
    if (records.length + failures.length + incomplete.length >= 100)
      throw new Error('group ancestry limit exceeded');
    const parents = run('git', ['show', '-s', '--format=%P', commit]).split(' ');
    if (parents.length !== 2) throw new Error(`unexplained group commit ${commit}`);
    try {
      const pulls = await pages(`commits/${parents[1]}/pulls`);
      const matches = pulls.filter(
        (pull) =>
          pull.head?.sha === parents[1] &&
          pull.state === 'open' &&
          pull.base?.ref === 'main' &&
          pull.base.repo?.full_name === repository
      );
      if (matches.length !== 1) throw new Error(`cannot resolve queued PR at ${parents[1]}`);
      if (
        !self.members?.some(
          (member) =>
            member.pr === matches[0].number &&
            member.head === parents[1] &&
            member.status === 'PASS'
        )
      )
        throw new Error(`fresh head approval absent for #${matches[0].number}`);
      records.push(await certified(matches[0].number, parents[1]));
    } catch (error) {
      (error.certStatus === 'FAIL' ? failures : incomplete).push(error.message);
    }
    commit = parents[0];
  }
  if (!records.length && !failures.length && !incomplete.length)
    incomplete.push('group has no queued PR records');
  const record = {
    schema: 1,
    repository,
    head,
    base,
    status: failures.length ? 'FAIL' : incomplete.length ? 'INCOMPLETE' : 'PASS',
    records,
    failures,
    incomplete,
    residual,
  };
  record.line = `cert-review-pr group: ${record.status} ${head} (${failures.length || incomplete.length ? [...failures, ...incomplete].join('; ') : records.map((item) => `#${item.pr}`).join(', ')})`;
  await publish(head, record);
  return record;
}
let record;
try {
  record = values.action === 'group' ? await group() : await pr();
} catch (error) {
  record = { status: 'INCOMPLETE', reason: error.message, residual };
  if (values.action === 'group' && /^[a-f0-9]{40}$/.test(event.merge_group?.head_sha ?? '')) {
    record.line = `cert-review-pr group: INCOMPLETE ${event.merge_group.head_sha} (${error.message})`;
    try {
      await publish(event.merge_group.head_sha, record);
    } catch (failure) {
      record.publicationError = failure.message;
    }
  }
}
if (values.out) save(values.out, record);
if (process.env.GITHUB_STEP_SUMMARY)
  writeFileSync(process.env.GITHUB_STEP_SUMMARY, `${record.line ?? record.reason}\n`);
console.log(JSON.stringify(record));
if (!['PASS', 'NOOP'].includes(record.status)) process.exitCode = 1;
printVerdict(record.status === 'NOOP' ? 'PASS' : record.status, `document=${record.status}`);
