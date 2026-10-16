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
      failureType: details?.error?.failureType,
    })}\n`;
  }
}
