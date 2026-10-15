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

export function browserArgs() {
  const hosts = [
    'seven.sliccy.ai',
    'vault.sliccy.ai',
    'vault-x.sliccy.ai',
    'sliccy.ai',
    'client.guard.test',
    'plain.guard.test',
    'same.guard.test',
    'cross.other.test',
  ];
  return [
    `--host-resolver-rules=${hosts.map((host) => `MAP ${host} 127.0.0.1`).join(', ')}`,
    '--no-proxy-server',
    '--disable-extensions',
    '--disable-component-extensions-with-background-pages',
    '--disable-background-networking',
    '--disable-sync',
    '--password-store=basic',
    '--use-mock-keychain',
    '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
  ];
}
