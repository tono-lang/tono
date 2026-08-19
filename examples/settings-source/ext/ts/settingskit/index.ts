// A stand-in for a real third-party settings source library that is generic
// over the value it resolves: the recipe binds against it declaratively (no
// bespoke code), so this file only exists to give the generated SDK something
// real, generic, to compile against. The generated declared test never reaches
// it: the test swaps the SDK's own seams for the constructor and the method.
export interface Source<T> {
  get(): Promise<T>;
}

export async function newEnvSource<T>(_service: string, _region: string): Promise<Source<T>> {
  return {
    get: async () => {
      throw new Error("the stand-in source resolves no value");
    },
  };
}
