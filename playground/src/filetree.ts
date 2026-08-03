/* Pure model for the generated-files tree: paths in, nested directories out.
   Rendering stays in main.ts; this shape is what the tests pin down. */

export interface TreeFile {
  name: string;
  /* Index into the flat GeneratedFile list, which stays the source of truth. */
  index: number;
}

export interface TreeDir {
  name: string;
  dirs: TreeDir[];
  files: TreeFile[];
}

/* Build a tree from slash-separated paths. The caller strips any prefix it
   does not want shown (the playground strips the target directory). Sibling
   directories sort before files, each alphabetically. */
export function buildTree(paths: string[]): TreeDir {
  const root: TreeDir = { name: "", dirs: [], files: [] };
  paths.forEach((path, index) => {
    const parts = path.split("/").filter(Boolean);
    let dir = root;
    for (const part of parts.slice(0, -1)) {
      let next = dir.dirs.find((d) => d.name === part);
      if (!next) {
        next = { name: part, dirs: [], files: [] };
        dir.dirs.push(next);
      }
      dir = next;
    }
    dir.files.push({ name: parts[parts.length - 1] ?? path, index });
  });
  const sortDir = (dir: TreeDir): void => {
    dir.dirs.sort((a, b) => a.name.localeCompare(b.name));
    dir.files.sort((a, b) => a.name.localeCompare(b.name));
    dir.dirs.forEach(sortDir);
  };
  sortDir(root);
  return root;
}

/* Drop the leading path segment (the per-target output directory) so the tree
   reads like the SDK layout the user would see on disk. */
export function stripTargetDir(path: string): string {
  const i = path.indexOf("/");
  return i === -1 ? path : path.slice(i + 1);
}
