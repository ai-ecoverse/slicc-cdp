export function guardConcurrency(targetCount, env = process.env) {
  const parallel = Number(env.SLICC_GUARD_PARALLEL ?? 4);
  const targetParallel = Number(env.SLICC_GUARD_TARGET_PARALLEL ?? Math.min(16, targetCount));
  if (
    ![targetCount, parallel, targetParallel].every((value) => Number.isInteger(value) && value > 0)
  )
    throw new RangeError('guard concurrency must use positive integers');
  return { parallel, targetParallel };
}

export function origins(fixture) {
  const origin = (host) => `http://${host}:${fixture.port}`;
  return {
    seven: origin('seven.sliccy.ai'),
    vault: origin('vault.sliccy.ai'),
    'vault-x': origin('vault-x.sliccy.ai'),
    apex: origin('sliccy.ai'),
    own: origin('client.guard.test'),
    client: origin('client.guard.test'),
    ordinary: origin('plain.guard.test'),
    cross: origin('cross.other.test'),
    'ordinary-same-site': origin('same.guard.test'),
  };
}

export function browserStartup(launch, report = () => {}) {
  let rowStarted = false;
  let retried = false;
  const retries = [];
  return {
    markRowStarted() {
      rowStarted = true;
    },
    get retries() {
      return retries.map((retry) => ({ ...retry }));
    },
    async start(args, details = {}) {
      try {
        return await launch(args);
      } catch (error) {
        if (
          rowStarted ||
          retried ||
          !/^debugging port \d+ did not answer \/json\/version\b/.test(error?.message ?? '')
        )
          throw error;
        retried = true;
        const retry = { ...details, attempt: 1, reason: error.message };
        retries.push(retry);
        report({ ...retry });
        return launch(args);
      }
    },
  };
}

export function browserArgs({ privileged = false } = {}) {
  const hosts = [
    'seven.sliccy.ai',
    'vault.sliccy.ai',
    'vault-x.sliccy.ai',
    'sliccy.ai',
    'client.guard.test',
    'plain.guard.test',
    'same.guard.test',
    'cross.other.test',
    'exa%20mple.sliccy.ai',
    'xn--1ug.sliccy.ai',
    'xn--a.sliccy.ai',
  ];
  return [
    `--host-resolver-rules=${hosts.map((host) => `MAP ${host} 127.0.0.1`).join(', ')}`,
    '--no-proxy-server',
    ...(privileged
      ? []
      : ['--disable-extensions', '--disable-component-extensions-with-background-pages']),
    '--disable-background-networking',
    '--disable-sync',
    '--password-store=basic',
    '--use-mock-keychain',
    `--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable${privileged ? '' : ',WebUIOmniboxPopup,WebUIOmniboxFullPopup,WebUIOmniboxAimPopup'}`,
  ];
}
