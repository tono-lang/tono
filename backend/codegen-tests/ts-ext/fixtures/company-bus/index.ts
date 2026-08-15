// A stand-in for the third-party bus library the generated SDK integrates
// with.

export type Publisher = unknown;

export async function connect(endpoint: string, token: string): Promise<Publisher> {
  return { endpoint, token };
}
