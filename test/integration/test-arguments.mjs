import './ports.mjs';

export const testArguments = [
  '--import',
  new URL('./ports.mjs', import.meta.url).href,
  '--test',
  '--test-timeout=600000',
];
