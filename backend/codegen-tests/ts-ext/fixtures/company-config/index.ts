// A stand-in for the third-party config library the generated SDK
// integrates with.

export interface TsOpts {
  region: string;
  service: string;
}

export interface TsConfig {
  host: string;
  token: string;
}

export async function load(opts: TsOpts): Promise<TsConfig> {
  return { host: `${opts.service}.internal`, token: "s3cr3t" };
}
