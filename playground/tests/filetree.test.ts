import { describe, expect, it } from "vitest";
import { buildTree, stripTargetDir } from "../src/filetree";

describe("buildTree", () => {
  it("nests directories and keeps flat-list indices", () => {
    const tree = buildTree(["playground/types.ts", "package.json", "playground/codec.ts"]);
    expect(tree.files).toEqual([{ name: "package.json", index: 1 }]);
    expect(tree.dirs).toHaveLength(1);
    expect(tree.dirs[0].name).toBe("playground");
    expect(tree.dirs[0].files).toEqual([
      { name: "codec.ts", index: 2 },
      { name: "types.ts", index: 0 },
    ]);
  });

  it("sorts sibling directories and files alphabetically", () => {
    const tree = buildTree(["b/x.go", "a/y.go", "z.go", "a.go"]);
    expect(tree.dirs.map((d) => d.name)).toEqual(["a", "b"]);
    expect(tree.files.map((f) => f.name)).toEqual(["a.go", "z.go"]);
  });

  it("handles deep nesting", () => {
    const tree = buildTree(["internal/descriptor/descriptor.go"]);
    expect(tree.dirs[0].name).toBe("internal");
    expect(tree.dirs[0].dirs[0].name).toBe("descriptor");
    expect(tree.dirs[0].dirs[0].files[0]).toEqual({ name: "descriptor.go", index: 0 });
  });

  it("is empty for no paths", () => {
    expect(buildTree([])).toEqual({ name: "", dirs: [], files: [] });
  });
});

describe("stripTargetDir", () => {
  it("drops the first segment only", () => {
    expect(stripTargetDir("typescript/playground/types.ts")).toBe("playground/types.ts");
    expect(stripTargetDir("rust/lib.rs")).toBe("lib.rs");
  });

  it("keeps a bare filename", () => {
    expect(stripTargetDir("lib.rs")).toBe("lib.rs");
  });
});
