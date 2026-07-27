// The two Node facilities the driver uses, declared here so the example needs
// no @types/node just to read a file and print a line.

declare module "node:fs" {
  export function readFileSync(path: string, encoding: string): string;
}

declare const process: {
  argv: string[];
  stdout: { write(s: string): void };
  stderr: { write(s: string): void };
  exit(code: number): never;
};
