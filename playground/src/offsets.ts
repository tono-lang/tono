/* The frontend reports spans as byte offsets into the UTF-8 source (OCaml
   strings), while CodeMirror addresses UTF-16 code units. Build a lookup once
   per document and translate every span through it. */
export function byteToCharMapper(source: string): (byte: number) => number {
  const map: number[] = [];
  let byte = 0;
  let i = 0;
  while (i < source.length) {
    const cp = source.codePointAt(i)!;
    const units = cp > 0xffff ? 2 : 1;
    const bytes = cp <= 0x7f ? 1 : cp <= 0x7ff ? 2 : cp <= 0xffff ? 3 : 4;
    for (let b = 0; b < bytes; b++) map[byte + b] = i;
    byte += bytes;
    i += units;
  }
  map[byte] = source.length;
  return (b: number) => {
    if (b < 0) return 0;
    if (b >= map.length) return source.length;
    return map[b];
  };
}
