// The TypeScript conformance driver: runs every vector through the generated
// client and prints one classified result per case, in the same closed
// vocabulary the Go driver uses. Comparing the two outputs is what proves the
// implementations agree; comparing either against the vectors' declared
// expectations is what proves they are both right.
//
// The client is driven, not the bespoke symbol directly: the point of a vector
// is the behavior a caller sees, which includes the generated glue.

import { readFileSync } from "node:fs";

import {
  APIError,
  ContractError,
  DecodeError,
  type Note,
  type NoteRef,
  OverloadedError,
  ValidationError,
} from "../../notes";
import { Client } from "../../notes";
import { encodeNote } from "../../notes/codec";

interface VectorFile {
  operation: string;
  cases: { name: string; input: unknown }[];
}

async function main(): Promise<void> {
  const paths = process.argv.slice(2);
  if (paths.length === 0) {
    throw new Error("usage: driver <vectors.json>...");
  }
  const client = new Client();
  const results: Record<string, unknown>[] = [];
  for (const path of paths) {
    const file = JSON.parse(readFileSync(path, "utf8")) as VectorFile;
    for (const c of file.cases) {
      const result = await run(client, file.operation, c.input);
      results.push({ ...result, name: c.name });
    }
  }
  process.stdout.write(JSON.stringify(results) + "\n");
}

async function run(
  client: Client,
  operation: string,
  input: unknown,
): Promise<Record<string, unknown>> {
  try {
    switch (operation) {
      case "save_note":
        return ok(await client.saveNote(fromWire(input)));
      case "archive_note":
        return ok(await client.archiveNote(input as NoteRef));
      default:
        throw new Error(`unknown operation ${operation}`);
    }
  } catch (e) {
    return classify(e);
  }
}

// ok renders a note in its serialized form, so a language's own field spelling
// (@rename) never leaks into the comparison.
function ok(value: Note): Record<string, unknown> {
  return { outcome: "ok", value: encodeNote(value) };
}

// The vectors carry the wire spelling; the client takes the language one.
function fromWire(input: unknown): Note {
  const raw = input as Record<string, string>;
  return { id: raw["id"], body: raw["body"], modified: raw["updated_at"] };
}

// classify reduces the call to the closed vocabulary both drivers share. The
// declared errors are named explicitly: this SDK has one, and spelling it out
// is what makes the typed-passthrough claim checkable.
function classify(e: unknown): Record<string, unknown> {
  if (e instanceof ValidationError) {
    return { outcome: "validation", fields: e.violations.map((v) => v.field) };
  }
  if (e instanceof OverloadedError) {
    return {
      outcome: "declared",
      code: "overloaded",
      retryable: e.retryable(),
      data: e.data,
    };
  }
  if (e instanceof DecodeError) {
    return { outcome: "decode", path: e.path };
  }
  if (e instanceof ContractError) {
    return { outcome: "contract", contract: e.contractName };
  }
  if (e instanceof APIError) {
    return { outcome: "api", status: e.status, body: e.body };
  }
  return { outcome: "unclassified", error: String(e) };
}

main().catch((e) => {
  process.stderr.write(String(e) + "\n");
  process.exit(1);
});
