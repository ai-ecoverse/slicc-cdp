import { registerHooks } from 'node:module';
import { createServer } from 'node:net';
import { mock } from 'node:test';
import { setTimeout as wallTimer } from 'node:timers';
import { pathToFileURL } from 'node:url';
import { syntheticReport } from './guard-test-evidence.mjs';

const simulated = process.env.TEARDOWN_MODE !== 'roundtrip';
if (simulated) mock.timers.enable({ apis: ['setTimeout', 'Date'], now: Date.now() });
const exhaust = (phase) =>
  wallTimer(() => mock.timers.tick(phase === 'cleanup' ? 115000 : 85000), 100);

const state = {
  async fixture() {
    if (process.env.TEARDOWN_MODE === 'fixture-hang') {
      exhaust('startup');
      return new Promise(() => {});
    }
    const server = createServer();
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    return {
      ...(process.env.TEARDOWN_MODE === 'registration' ? {} : { redirectSecret: async () => {} }),
      close: () => new Promise((resolve) => server.close(resolve)),
    };
  },
  async browser() {
    return {
      wsUrl: 'ws://teardown.invalid',
      async close() {
        if (process.env.TEARDOWN_MODE === 'hang') {
          exhaust('cleanup');
          return new Promise(() => {});
        }
        throw new Error('forced browser disposal rejection');
      },
    };
  },
  async connection(role) {
    if (process.env.TEARDOWN_MODE === `${role}-hang`) {
      exhaust('startup');
      return new Promise(() => {});
    }
    return { events: [], send: async () => ({}), close: async () => {} };
  },
  async run(set, options) {
    if (process.env.TEARDOWN_MODE === 'roundtrip') return { results: syntheticReport(set).results };
    const pair = await options.clientFactory({
      slot: 0,
      variant: 'prompt',
      detachDelay: 0,
      vector: set.vectors[0],
    });
    if (process.env.TEARDOWN_MODE === 'falsy') throw 0;
    const row = {
      id: 'E4.teardown',
      target: 'ordinary',
      variant: 'prompt',
      control: 'pass',
      guarded: 'pass',
      live: 'pass',
    };
    options.log(row);
    await pair.close();
    return { results: [row] };
  },
};
globalThis.__guardTeardownFixture = state;
const publishedRunner = import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-runner');
const runnerExports = `export {summary} from ${JSON.stringify(publishedRunner)};export const run=(...args)=>globalThis.__guardTeardownFixture.run(...args);`;

const modules = new Map([
  ['./chrome-policy.mjs', 'export function requireChrome() {}'],
  ['node:child_process', 'export function execFileSync(){return "fake-owned-worker";}'],
  [
    '@ai-ecoverse/slicc-browsers/fixture',
    'export const fixtureServer=()=>globalThis.__guardTeardownFixture.fixture();',
  ],
  ['@ai-ecoverse/slicc-browsers/guard-runner', runnerExports],
  [
    './browser.mjs',
    'export const launchBrowser=()=>globalThis.__guardTeardownFixture.browser();export const rawSocket=()=>Promise.reject(new Error("unrequested browser discovery"));',
  ],
  [
    './guard-client.mjs',
    'export const observer=()=>globalThis.__guardTeardownFixture.connection("observer");export const guarded=()=>globalThis.__guardTeardownFixture.connection("client");',
  ],
  [
    './guard-fixture.mjs',
    'export const browserArgs=()=>[];export const origins=()=>({client:"https://teardown.invalid"});export const browserStartup=(start)=>({start,retries:[],markRowStarted(){}});',
  ],
]);

registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith('/guard-set.mjs')) {
      if (specifier === '@ai-ecoverse/slicc-browsers/guard-vectors.json')
        return { url: pathToFileURL(process.env.TEARDOWN_VECTORS).href, shortCircuit: true };
      if (modules.has(specifier)) return { url: `guard-teardown:${specifier}`, shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    if (url.startsWith('guard-teardown:'))
      return {
        format: 'module',
        source: modules.get(url.slice('guard-teardown:'.length)),
        shortCircuit: true,
      };
    return nextLoad(url, context);
  },
});
process.stdout.write('FAKE-WORKER-READY\n');
