// A stand-in for the third-party bus library the generated SDK integrates
// with.

// The library's own error class: what a generated SDK recognizes with
// `instanceof`.
export class BusyError extends Error {
  constructor() {
    super("BUSY");
    this.name = "BusyError";
  }
}

export class Publisher {
  constructor(readonly endpoint: string, readonly token: string) {}

  async send(topic: string, body: string): Promise<{ ok: boolean }> {
    if (topic === "") {
      throw new BusyError();
    }
    return { ok: true };
  }
}

export async function connect(endpoint: string, token: string): Promise<unknown> {
  return new Publisher(endpoint, token);
}
