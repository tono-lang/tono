// A stand-in for the third-party bus library the generated SDK integrates
// with.

export class Publisher {
  constructor(readonly endpoint: string, readonly token: string) {}

  async send(topic: string, body: string): Promise<{ ok: boolean }> {
    if (topic === "") {
      throw new Error("BUSY");
    }
    return { ok: true };
  }
}

export async function connect(endpoint: string, token: string): Promise<unknown> {
  return new Publisher(endpoint, token);
}
