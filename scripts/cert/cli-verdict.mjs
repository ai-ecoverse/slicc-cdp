import { writeSync } from 'node:fs';

export function cliVerdict(prefix, refusal = 'early refusal') {
  let printed = false;
  const print = (status, detail = '') => {
    if (printed) return;
    printed = true;
    const word = ['PASS', 'FAIL', 'INCOMPLETE'].includes(status) ? status : 'INCOMPLETE';
    writeSync(1, `${prefix}${word} ${String(detail).replace(/[\r\n]/g, ' ')}\n`);
  };
  process.once('uncaughtExceptionMonitor', (error) =>
    print(error.certStatus === 'FAIL' ? 'FAIL' : 'INCOMPLETE', refusal)
  );
  process.once('exit', () => print('INCOMPLETE', refusal));
  return print;
}
