function errorDetails(error) {
  const seen = new Set();
  let nodes = 0;
  const visit = (value, depth) => {
    if (value === null || value === undefined) return value;
    if (typeof value !== 'object') return { message: String(value) };
    if (seen.has(value)) return { omitted: 'cycle or repeated error reference' };
    if (depth > 8 || ++nodes > 128) return { omitted: 'error tree limit' };
    seen.add(value);
    const result = {};
    for (const key of ['name', 'message', 'code', 'stack', 'failureType']) {
      if (value[key] === undefined) continue;
      const text = String(value[key]);
      result[key] = text.slice(0, 16000);
      if (text.length > 16000) (result.omittedCharacters ??= {})[key] = text.length - 16000;
    }
    if (value.cause !== undefined) result.cause = visit(value.cause, depth + 1);
    if (Array.isArray(value.errors)) {
      result.errors = value.errors.slice(0, 64).map((nested) => visit(nested, depth + 1));
      if (value.errors.length > 64) result.omittedErrors = value.errors.length - 64;
    }
    return result;
  };
  return visit(error, 0);
}

export default async function* reporter(events) {
  for await (const event of events) {
    if (['test:stdout', 'test:stderr', 'test:diagnostic'].includes(event.type)) {
      process.stderr.write(`${event.data.file ?? ''}: ${event.data.message ?? ''}\n`);
      continue;
    }
    if (!['test:pass', 'test:fail', 'test:summary', 'test:plan'].includes(event.type)) continue;
    const { name, file, nesting, skip, todo, details, success, counts } = event.data;
    yield `${JSON.stringify({
      type: event.type,
      name,
      file,
      nesting,
      skip,
      todo,
      success,
      counts,
      error: details?.error?.message,
      errorDetails: errorDetails(details?.error),
      failureType: details?.error?.failureType,
    })}\n`;
  }
}
