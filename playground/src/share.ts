/* Snippet sharing: the source is deflated and base64url-encoded into the URL
   fragment, so links carry the whole snippet and the server (a static host)
   never sees it. Uses the browser's native CompressionStream: no dependency. */
import type { Target } from "./types";

export interface SharedState {
  source: string;
  target: Target;
  /* Path of the generated file that was open, so a shared link restores the
     whole view. Optional: absent on old links and when nothing generated. */
  file?: string;
  /* Run panel content, present when the panel was opened. Compressed like the
     source so a shared experiment carries its snippet and mocks. */
  run?: string;
  mocks?: string;
  /* Module name, when not the default. */
  name?: string;
}

async function pipeThrough(
  bytes: Uint8Array,
  stream: CompressionStream | DecompressionStream,
): Promise<Uint8Array> {
  const blob = new Blob([bytes as BlobPart]);
  const out = blob.stream().pipeThrough(stream);
  return new Uint8Array(await new Response(out).arrayBuffer());
}

function toBase64Url(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function fromBase64Url(text: string): Uint8Array {
  const b64 = text.replace(/-/g, "+").replace(/_/g, "/");
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

async function deflateParam(text: string): Promise<string> {
  const deflated = await pipeThrough(
    new TextEncoder().encode(text),
    new CompressionStream("deflate-raw"),
  );
  return toBase64Url(deflated);
}

async function inflateParam(value: string): Promise<string> {
  const inflated = await pipeThrough(
    fromBase64Url(value),
    new DecompressionStream("deflate-raw"),
  );
  return new TextDecoder().decode(inflated);
}

export async function encodeShareHash(state: SharedState): Promise<string> {
  const parts = [`code=${await deflateParam(state.source)}`, `target=${state.target}`];
  if (state.file) parts.push(`file=${encodeURIComponent(state.file)}`);
  if (state.name) parts.push(`name=${encodeURIComponent(state.name)}`);
  if (state.run !== undefined) parts.push(`run=${await deflateParam(state.run)}`);
  if (state.mocks !== undefined) parts.push(`mocks=${await deflateParam(state.mocks)}`);
  return `#${parts.join("&")}`;
}

export async function decodeShareHash(hash: string): Promise<SharedState | null> {
  const params = new URLSearchParams(hash.replace(/^#/, ""));
  const code = params.get("code");
  if (!code) return null;
  try {
    const source = await inflateParam(code);
    const target = params.get("target");
    const file = params.get("file");
    const run = params.get("run");
    const mocks = params.get("mocks");
    const name = params.get("name");
    return {
      source,
      target: target === "rust" || target === "go" ? target : "ts",
      ...(file ? { file } : {}),
      ...(name ? { name } : {}),
      ...(run !== null ? { run: await inflateParam(run) } : {}),
      ...(mocks !== null ? { mocks: await inflateParam(mocks) } : {}),
    };
  } catch {
    return null;
  }
}
