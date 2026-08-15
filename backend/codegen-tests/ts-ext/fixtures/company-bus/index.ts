// A stand-in for the third-party bus library the generated SDK integrates
// with.

export async function connect(endpoint: string, token: string): Promise<unknown> {
  return { endpoint, token };
}
