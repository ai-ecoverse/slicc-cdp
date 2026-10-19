import { execFileSync } from 'node:child_process';
import { appendFileSync } from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { cliVerdict } from './cli-verdict.mjs';
import { json, root, run, save } from './common.mjs';
import { client, hash, residual, reviewerPaths } from './pr-common.mjs';

const printVerdict = cliVerdict('pr-plan: ', 'go=false document=unavailable early refusal');
const { values } = parseArgs({
  options: {
    event: { type: 'string', default: process.env.GITHUB_EVENT_PATH },
    out: { type: 'string' },
    directory: { type: 'string', default: root },
    merge: { type: 'string' },
    self: { type: 'string' },
  },
});
const event = json(values.event);
const pull = event.pull_request;
const repository = event.repository.full_name;
const { api, pages } = client(repository);
const sha = /^[a-f0-9]{40}$/;
const plan = {
  schema: 1,
  repository,
  pr: pull.number,
  head: pull.head.sha,
  tools: pull.base.sha,
  mode: 'pr',
  harness: pull.base.sha,
  run: process.env.GITHUB_RUN_ID ?? 'local',
  attempt: process.env.GITHUB_RUN_ATTEMPT ?? '1',
  go: false,
  report: true,
  status: 'INCOMPLETE',
  testOnly: false,
  issues: [],
  residual,
};
try {
  if (!sha.test(plan.head) || !sha.test(plan.tools)) throw new Error('invalid PR identity');
  if (
    ['labeled', 'unlabeled'].includes(event.action) &&
    event.label?.name !== 'cert-review-approved'
  ) {
    plan.report = false;
    plan.reason = 'unrelated label';
  } else {
    const current = await api(`pulls/${plan.pr}`);
    if (current.head.sha !== plan.head || current.state !== 'open') {
      plan.report = false;
      plan.reason = 'stale head or closed PR';
    } else {
      if (current.base.ref !== 'main' || current.base.repo.full_name !== repository)
        throw new Error('PR does not target repository main');
      const files = await pages(`pulls/${plan.pr}/files`);
      if (
        files.length !== current.changed_files ||
        new Set(files.map((file) => file.filename)).size !== files.length
      )
        throw new Error('changed-file listing incomplete');
      const paths = [
        ...new Set(
          files.flatMap((file) => [file.filename, file.previous_filename].filter(Boolean))
        ),
      ];
      if (
        paths.some(
          (file) =>
            typeof file !== 'string' ||
            file.startsWith('/') ||
            file.split('/').includes('..') ||
            /[\r\n\0]/.test(file)
        )
      )
        throw new Error('invalid changed path');
      Object.assign(plan, {
        files,
        paths,
        reviewer: reviewerPaths(paths),
        testOnly: paths.length > 0 && paths.every((file) => file.startsWith('test/')),
      });
      plan.behaviour = paths.some(
        (file) => !/^(README|AGENTS)\.md$/.test(file) && !/^docs\/.*\.md$/.test(file)
      );
      plan.changedTests = files
        .filter(
          (file) =>
            file.status !== 'removed' && /^test\/integration\/[^/]+\.test\.mjs$/.test(file.filename)
        )
        .map((file) => file.filename);
      plan.self = values.self ? json(values.self) : null;
      if (
        plan.self?.status !== 'PASS' ||
        plan.self.repository !== repository ||
        !plan.self.members?.some(
          (member) => member.pr === plan.pr && member.head === plan.head && member.status === 'PASS'
        )
      ) {
        plan.status = plan.self?.status === 'FAIL' ? 'FAIL' : 'INCOMPLETE';
        plan.reason = 'cert-review: needs coordinator review';
      } else if (!plan.behaviour) {
        plan.status = 'PASS';
        plan.reason = 'not behaviour';
      } else if (
        current.head.repo?.full_name !== repository ||
        pull.head.repo?.full_name !== repository
      ) {
        plan.reason = 'refused: fork behaviour requires trusted same-repository route';
      } else if (current.draft !== false) {
        plan.reason = 'not reviewed: draft, mark it ready for review';
      } else {
        if (!sha.test(values.merge ?? '')) throw new Error('merge ref unavailable');
        const parents = run('git', ['show', '-s', '--format=%P', values.merge], {
          cwd: values.directory,
        }).split(' ');
        if (parents.length !== 2 || parents[1] !== plan.head)
          throw new Error('merge ref does not bind PR head');
        run('git', ['merge-base', '--is-ancestor', plan.tools, parents[0]], {
          cwd: values.directory,
        });
        Object.assign(plan, {
          base: parents[0],
          commit: values.merge,
          version: `0.0.0-pr.${plan.pr}.${plan.run}`,
          baseline: { version: `0.0.0-base.${plan.pr}.${plan.run}`, commit: parents[0] },
        });
        const manifest = JSON.parse(
          run('git', ['show', `${plan.commit}:package.json`], { cwd: values.directory })
        );
        plan.rust = run('git', ['show', `${plan.commit}:rust-toolchain.toml`], {
          cwd: values.directory,
        }).match(/^channel = "(\d+\.\d+\.\d+)"$/m)?.[1];
        plan.vectors = json(path.join(root, 'package.json')).devDependencies[
          '@ai-ecoverse/slicc-browsers'
        ];
        plan.chrome = manifest.config.chrome;
        if (!plan.rust || !/^\d+\.\d+\.\d+$/.test(plan.vectors))
          throw new Error('exact Rust/vector pins unavailable');
        plan.go = true;
        plan.reason = 'behaviour review';
        plan.lockSha256 = hash(
          execFileSync('git', ['show', `${plan.commit}:Cargo.lock`], { cwd: values.directory })
        );
      }
    }
  }
} catch (error) {
  plan.issues.push(error.message);
  plan.reason = 'planning evidence unavailable';
}
save(values.out, plan);
if (process.env.GITHUB_OUTPUT)
  appendFileSync(
    process.env.GITHUB_OUTPUT,
    `go=${plan.go}\nreport=${plan.report}\nversion=${plan.version ?? ''}\nrust=${plan.rust ?? ''}\nhead=${plan.head}\ncommit=${plan.commit ?? ''}\nbase=${plan.base ?? ''}\ntest_only=${plan.go && plan.testOnly}\n`
  );
console.log(JSON.stringify(plan));
printVerdict(
  plan.go || plan.status === 'PASS' ? 'PASS' : plan.status,
  `go=${plan.go} document=${plan.status}`
);
