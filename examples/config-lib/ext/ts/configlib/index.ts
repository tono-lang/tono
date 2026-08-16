// A stand-in for a real third-party config library: the recipe binds
// against it declaratively (no bespoke code), so this file only exists to
// give the generated SDK something real to compile against.
export interface Config {
  host: string;
  token: string;
}

export async function load(service: string, region: string): Promise<Config> {
  return { host: `${service}.${region}.internal`, token: "s3cr3t" };
}
