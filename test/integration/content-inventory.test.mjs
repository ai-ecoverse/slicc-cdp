import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const literals = (text) =>
  [...text.matchAll(/"([A-Z][A-Za-z0-9]*\.[a-zA-Z][a-zA-Z0-9]*)"/g)].map((match) => match[1]);
const events = new Set([
  'Audits.issueAdded',
  'CSS.styleSheetAdded',
  'DOM.documentUpdated',
  'Debugger.paused',
  'Debugger.scriptFailedToParse',
  'Debugger.scriptParsed',
  'Fetch.authRequired',
  'Fetch.requestPaused',
  'HeapProfiler.addHeapSnapshotChunk',
  'Inspector.detached',
  'Inspector.targetCrashed',
  'Log.entryAdded',
  'Network.loadingFailed',
  'Network.loadingFinished',
  'Network.requestWillBeSent',
  'Network.requestWillBeSentExtraInfo',
  'Network.responseReceived',
  'Network.responseReceivedExtraInfo',
  'Network.webSocketCreated',
  'Page.frameAttached',
  'Page.frameDetached',
  'Page.frameNavigated',
  'Page.frameRequestedNavigation',
  'Page.frameStartedNavigating',
  'Page.javascriptDialogClosed',
  'Page.javascriptDialogOpening',
  'Page.lifecycleEvent',
  'Page.loadEventFired',
  'Page.navigatedWithinDocument',
  'Page.screencastFrame',
  'Runtime.bindingCalled',
  'Runtime.consoleAPICalled',
  'Runtime.exceptionThrown',
  'Runtime.executionContextCreated',
  'Runtime.executionContextDestroyed',
  'Runtime.executionContextsCleared',
  'Runtime.inspectRequested',
  'ServiceWorker.workerVersionUpdated',
  'Target.attachedToTarget',
  'Target.detachedFromTarget',
  'Target.receivedMessageFromTarget',
  'Target.targetCreated',
  'Target.targetDestroyed',
  'Target.targetInfoChanged',
]);
const nonMethods = new Set([
  'AbortSignal.timeout',
  'EXAMPLE.com',
  'Info.plist',
  'JSON.stringify',
  'README.md',
  'SKILL.md',
]);

async function sources(directory) {
  const out = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) out.push(...(await sources(file)));
    else if (entry.name.endsWith('.rs')) out.push(file);
  }
  return out;
}

test('every Rust CLI and client method has an explicit content policy', async () => {
  const client = await readFile(path.join(root, 'crates/cdp-client/src/cdp.rs'), 'utf8');
  const policy = client.split('fn content_command(', 2)[1]?.split('\nfn harmless(', 1)[0];
  assert.ok(policy, 'the content classifier is required');
  const refused = policy.match(/match method \{([\s\S]*?)=> None,/)?.[1];
  const content = policy.match(/=> None,([\s\S]*?)=> Some\(true\),/)?.[1];
  const control = policy.match(/=> Some\(true\),([\s\S]*?)=> Some\(false\),/)?.[1];
  assert.ok(
    refused && content && control,
    'explicit refusal, content and control arms are required'
  );
  assert.match(policy, /if method == "Page\.addScriptToEvaluateOnNewDocument" \{\s*return Some\(/);
  const classified = new Set([
    ...literals(refused),
    ...literals(content),
    ...literals(control),
    'Page.addScriptToEvaluateOnNewDocument',
  ]);
  const missing = [];
  const used = new Set();
  for (const file of await sources(path.join(root, 'crates'))) {
    for (const method of literals(await readFile(file, 'utf8'))) {
      if (events.has(method) || nonMethods.has(method)) continue;
      used.add(method);
      if (!classified.has(method)) missing.push(`${path.relative(root, file)}: ${method}`);
    }
  }
  assert.ok(used.has('Runtime.evaluate'));
  assert.ok(used.has('Fetch.fulfillRequest'));
  assert.ok(used.has('Target.createTarget'));
  assert.deepEqual(
    [...new Set(missing)].sort(),
    [],
    'classify new method literals before sending them'
  );
});
