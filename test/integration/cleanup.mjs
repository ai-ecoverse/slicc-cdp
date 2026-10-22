export async function deadline(task, phase, ms = 5000, abort = () => {}) {
  let timer;
  try {
    return await Promise.race([
      Promise.resolve().then(task),
      new Promise((_, reject) => {
        timer = setTimeout(() => {
          const error = Object.assign(new Error(`${phase} deadline exceeded`), {
            code: 'HARNESS_AVAILABILITY',
            phase,
          });
          reject(error);
          Promise.resolve()
            .then(() => abort(error))
            .catch((cause) => {
              error.cause = cause;
            });
        }, ms);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

export async function cleanup(tasks, ...failures) {
  const results = await Promise.allSettled(
    tasks.map((task, index) => deadline(task, `cleanup-${index}`))
  );
  const errors = [
    ...failures,
    ...results.filter((result) => result.status === 'rejected').map((result) => result.reason),
  ];
  if (errors.length) throw new AggregateError(errors, 'harness execution or cleanup failed');
}

export function normalizeError(value, seen = new Set()) {
  let message;
  try {
    message = String(value);
  } catch {
    message = 'unprintable rejection';
  }
  const result = { type: value === null ? 'null' : typeof value, message };
  if ((typeof value !== 'object' || value === null) && typeof value !== 'function') return result;
  if (seen.has(value) || seen.size >= 32) return { ...result, recursive: true };
  seen.add(value);
  const read = (key) => {
    try {
      return value[key];
    } catch {
      return undefined;
    }
  };
  for (const key of ['name', 'message', 'stack', 'code', 'phase']) {
    const field = read(key);
    if (typeof field === 'string') result[key] = field;
  }
  const errors = read('errors');
  try {
    if (Array.isArray(errors))
      result.errors = Array.from(errors, (error) => normalizeError(error, seen));
  } catch {
    result.errorsUnavailable = true;
  }
  const cause = read('cause');
  if (cause !== undefined) result.cause = normalizeError(cause, seen);
  return result;
}
