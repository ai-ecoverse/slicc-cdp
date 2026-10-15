import { chromiumSuite } from './chromium-flows.mjs';

chromiumSuite('chromium through the native binaries and the kernel', [
  'native playwright-cli',
  'native basics batch',
  'native curlwright',
  'native route',
  'native protected tabs',
  'native navigation guard',
  'native upload validation',
  'kernel playwright-cli',
  'kernel curlwright',
  'kernel route',
]);
