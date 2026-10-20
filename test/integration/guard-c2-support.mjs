export const redirectIds = ['C2.redirect-secret', 'C2.redirect-secret-mix', 'C2.redirect-body'];

export function redirectSelection(published) {
  const vectors = redirectIds.map((id) => {
    const matches = published.vectors.filter((vector) => vector.id === id);
    if (matches.length !== 1) throw new Error(`expected exactly one published row ${id}`);
    return matches[0];
  });
  return { ...published, vectors };
}

export async function recordedClient(open) {
  const messages = [];
  const client = await open((message) => messages.push(JSON.stringify(message.event ?? message)));
  client.messages = messages;
  return client;
}

export function requireRedirectRegistration(fixture) {
  if (typeof fixture.redirectSecret !== 'function')
    throw new Error('redirectSecret fixture registration required');
  return fixture;
}
