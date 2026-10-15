import { chromiumSuite } from './chromium-flows.mjs';

chromiumSuite('chromium guards and uploads through the kernel', [
  'kernel protected tabs',
  'kernel navigation guard',
  'kernel upload vfs isolation',
]);
