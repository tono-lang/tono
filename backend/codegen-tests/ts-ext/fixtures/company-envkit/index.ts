// A stand-in for a third-party settings-provider library: a provider
// constructed once, whose methods resolve the endpoints several operations
// of the generated SDK read.

export interface Config {
  readUrl: string;
  writeUrl: string;
}

export class Provider {
  constructor(readonly name: string) {}

  async get(): Promise<Config> {
    return { readUrl: `https://read.${this.name}`, writeUrl: `https://write.${this.name}` };
  }

  async getFor(region: string): Promise<Config> {
    return {
      readUrl: `https://${region}.read.${this.name}`,
      writeUrl: `https://${region}.write.${this.name}`,
    };
  }
}

export async function newProvider(name: string): Promise<unknown> {
  return new Provider(name);
}
