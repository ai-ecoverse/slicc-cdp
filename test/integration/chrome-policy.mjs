import test from 'node:test';
import { trackChromeCase } from './chrome-report.mjs';

export const CHROME_REFUSAL = 'real Chrome is CI-only; set SLICC_LOCAL_CHROME=1 to run it locally';

export function chromeSkipReason(env = process.env) {
  return env.CI || env.SLICC_LOCAL_CHROME === '1' ? false : CHROME_REFUSAL;
}

export function requireChrome(env = process.env) {
  const reason = chromeSkipReason(env);
  if (reason) throw new Error(reason);
}

function argumentsFor(options, fn) {
  return typeof options === 'function' ? [{}, options] : [options, fn];
}

function tracked(fn, ci) {
  return (context) =>
    trackChromeCase(async (context) => {
      if (ci) {
        const nested = context.test.bind(context);
        context.test = (name, options, fn) => {
          const [settings, callback] = argumentsFor(options, fn);
          return nested(name, { ...settings, skip: false }, tracked(callback, ci));
        };
        context.skip = () => {
          throw new Error('real Chrome cases must not skip on CI');
        };
      }
      return fn(context);
    }, context);
}

export function chromeTestArguments(options, fn) {
  const [settings, callback] = argumentsFor(options, fn);
  const ci = Boolean(process.env.CI);
  const skip = ci ? false : chromeSkipReason() || settings.skip;
  return [{ ...settings, skip }, tracked(callback, ci)];
}

export function chromeTest(name, options, fn) {
  return test(name, ...chromeTestArguments(options, fn));
}
