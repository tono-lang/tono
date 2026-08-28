//! The symbol index on disk: one JSON file per (ext, language), the neutral
//! shape every extractor writes and the language server reads.
//!
//! The file is the whole contract between the two sides. An extractor for a
//! new language produces this shape and nothing else changes; the server
//! reads it directly, so the completion path never waits on a process. The
//! index is never a verdict: a symbol it lists may still fail the binding
//! check, and a symbol it misses is a suggestion that does not appear, never
//! a false ok. That is why the reader tolerates an unknown kind and why a
//! stale file is discarded rather than trusted: the `key` records exactly
//! what the index was built from, and a reader that computes a different key
//! from what is on disk now treats the file as absent.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The index file format version. A reader that understands a different
/// version discards the file; the builder rewrites it on the next request.
pub(crate) const FORMAT: u32 = 1;

/// The digest recorded when the lockfile is absent. Recorded together with
/// the path the lockfile would have, so its later appearance is a change.
pub(crate) const NO_LOCKFILE: &str = "none";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Index {
    pub tono_index_version: u32,
    pub key: Key,
    /// A limitation of the extractor worth telling the user (an API a macro
    /// produces, a re-export it could not follow), shown in the editor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub symbols: Vec<Symbol>,
}

/// What the index was built from. Every field is something the reader can
/// recompute from the project as it is now, which is how a stale index is
/// told apart from a current one without a timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Key {
    pub ext: String,
    pub lang: String,
    /// The library as the `.tono` spells it (`go { #(path) }`).
    pub package: String,
    /// The version pinned in the manifest's `[ext.<name>]` table.
    pub version: String,
    pub lockfile: Lockfile,
    pub format: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Lockfile {
    pub path: String,
    pub digest: String,
}

impl Lockfile {
    pub(crate) fn none() -> Self {
        Lockfile {
            path: String::new(),
            digest: NO_LOCKFILE.to_string(),
        }
    }

    /// The lockfile at `path`, digested now; absent reads as [`NO_LOCKFILE`]
    /// with the path kept.
    pub(crate) fn at(path: &Path) -> Self {
        Lockfile {
            path: path.to_string_lossy().into_owned(),
            digest: digest_file(path),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SymbolKind {
    Function,
    Class,
    Struct,
    Interface,
    Type,
    Enum,
    Const,
    Namespace,
    Trait,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MemberKind {
    Method,
    Field,
    Constructor,
    Function,
    Type,
    Const,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Symbol {
    /// The name a spelling would use after the library's own qualifier:
    /// bare for Go and TypeScript, `module::Name` for a Rust item below the
    /// crate root.
    pub name: String,
    pub kind: SymbolKind,
    /// One entry per overload, so a name with several signatures stays one
    /// completion item.
    #[serde(default)]
    pub signatures: Vec<String>,
    #[serde(default)]
    pub doc: String,
    #[serde(default)]
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Member {
    pub name: String,
    pub kind: MemberKind,
    #[serde(rename = "static", default)]
    pub is_static: bool,
    #[serde(default)]
    pub signatures: Vec<String>,
}

impl Index {
    pub(crate) fn new(key: Key, note: Option<String>, symbols: Vec<Symbol>) -> Self {
        let mut index = Index {
            tono_index_version: FORMAT,
            key,
            note,
            symbols,
        };
        index.normalize();
        index
    }

    /// The same set of symbols always serializes to the same bytes, whatever
    /// order an extractor found them in: symbols by name, members with the
    /// static ones first, duplicates (a name an extractor reached twice)
    /// collapsed onto the first.
    pub(crate) fn normalize(&mut self) {
        self.symbols.sort_by(|a, b| a.name.cmp(&b.name));
        self.symbols.dedup_by(|a, b| a.name == b.name);
        for symbol in &mut self.symbols {
            symbol
                .members
                .sort_by(|a, b| b.is_static.cmp(&a.is_static).then(a.name.cmp(&b.name)));
            symbol
                .members
                .dedup_by(|a, b| a.name == b.name && a.is_static == b.is_static);
        }
    }
}

/// FNV-1a over 64 bits: a few lines that read the same in every language the
/// server is written in, which is what a digest both sides must agree on
/// needs; it is not a security primitive and is not used as one.
pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub(crate) fn digest_hex(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a64(bytes))
}

/// The digest of a file's bytes, [`NO_LOCKFILE`] when it cannot be read.
pub(crate) fn digest_file(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => digest_hex(&bytes),
        Err(_) => NO_LOCKFILE.to_string(),
    }
}

/// Where the index of an (ext, language) pair lives: beside the manifest,
/// under a directory the project's `.gitignore` covers.
pub(crate) fn index_path(manifest_dir: &Path, ext: &str, lang: &str) -> PathBuf {
    manifest_dir
        .join(".tono")
        .join("index")
        .join(format!("{ext}.{lang}.json"))
}

/// Write the index in one move: the bytes land in a sibling file and are
/// renamed over the target, so a reader never sees a half-written index.
pub(crate) fn write(index: &Index, path: &Path) -> Result<(), String> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let bytes = serde_json::to_vec(index).map_err(|e| e.to_string())?;
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

#[cfg(test)]
pub(crate) fn read(path: &Path) -> Result<Index, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample_key() -> Key {
        Key {
            ext: "gearbox".into(),
            lang: "go".into(),
            package: "example.test/gearbox".into(),
            version: "v0.0.0".into(),
            lockfile: Lockfile {
                path: "/p/go.sum".into(),
                digest: "0123456789abcdef".into(),
            },
            format: FORMAT,
        }
    }

    pub(crate) fn sample_index() -> Index {
        Index::new(
            sample_key(),
            Some("a note".into()),
            vec![
                Symbol {
                    name: "Open".into(),
                    kind: SymbolKind::Function,
                    signatures: vec!["func(name string) *Dial".into(), "func() *Dial".into()],
                    doc: "Open a dial.".into(),
                    members: vec![],
                },
                Symbol {
                    name: "Dial".into(),
                    kind: SymbolKind::Struct,
                    signatures: vec![],
                    doc: String::new(),
                    members: vec![
                        Member {
                            name: "Read".into(),
                            kind: MemberKind::Method,
                            is_static: false,
                            signatures: vec!["func() (float64, error)".into()],
                        },
                        Member {
                            name: "New".into(),
                            kind: MemberKind::Constructor,
                            is_static: true,
                            signatures: vec!["(name: string): Dial".into()],
                        },
                        Member {
                            name: "Width".into(),
                            kind: MemberKind::Field,
                            is_static: false,
                            signatures: vec!["int".into()],
                        },
                    ],
                },
            ],
        )
    }

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tono-index-fmt-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_sample_round_trips_through_json() {
        let index = sample_index();
        let text = serde_json::to_string(&index).unwrap();
        let back: Index = serde_json::from_str(&text).unwrap();
        assert_eq!(back, index);
        // The wire spellings the reader keys on.
        assert!(text.contains("\"tono_index_version\":1"), "{text}");
        assert!(text.contains("\"kind\":\"struct\""), "{text}");
        assert!(text.contains("\"kind\":\"constructor\""), "{text}");
        assert!(text.contains("\"static\":true"), "{text}");
    }

    #[test]
    fn a_missing_note_and_empty_lists_read_back() {
        let text = r#"{"tono_index_version":1,"key":{"ext":"a","lang":"ts","package":"p","version":"1","lockfile":{"path":"","digest":"none"},"format":1},"symbols":[{"name":"X","kind":"class"}]}"#;
        let index: Index = serde_json::from_str(text).unwrap();
        assert_eq!(index.note, None);
        assert_eq!(index.symbols[0].members, vec![]);
        assert_eq!(index.symbols[0].signatures, Vec::<String>::new());
    }

    #[test]
    fn normalize_orders_symbols_and_members_and_drops_duplicates() {
        let mut index = sample_index();
        index.symbols.push(Symbol {
            name: "Dial".into(),
            kind: SymbolKind::Struct,
            signatures: vec![],
            doc: "second".into(),
            members: vec![],
        });
        index.normalize();
        let names: Vec<&str> = index.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Dial", "Open"]);
        let members: Vec<(&str, bool)> = index.symbols[0]
            .members
            .iter()
            .map(|m| (m.name.as_str(), m.is_static))
            .collect();
        assert_eq!(
            members,
            vec![("New", true), ("Read", false), ("Width", false)]
        );
    }

    #[test]
    fn fnv1a_matches_the_published_vectors() {
        assert_eq!(digest_hex(b""), "cbf29ce484222325");
        assert_eq!(digest_hex(b"a"), "af63dc4c8601ec8c");
        assert_eq!(digest_hex(b"foobar"), "85944171f73967e8");
    }

    #[test]
    fn digest_file_reads_none_for_an_absent_file() {
        let dir = tmp("digest");
        assert_eq!(digest_file(&dir.join("missing")), NO_LOCKFILE);
        std::fs::write(dir.join("lock"), "foobar").unwrap();
        assert_eq!(digest_file(&dir.join("lock")), "85944171f73967e8");
        let lock = Lockfile::at(&dir.join("lock"));
        assert!(lock.path.ends_with("lock"));
        assert_eq!(lock.digest, "85944171f73967e8");
        assert_eq!(Lockfile::none().digest, NO_LOCKFILE);
    }

    #[test]
    fn the_index_path_is_under_the_manifest_dir() {
        assert_eq!(
            index_path(Path::new("/proj"), "gearbox", "ts"),
            PathBuf::from("/proj/.tono/index/gearbox.ts.json")
        );
    }

    #[test]
    fn write_then_read_gives_the_index_back_and_leaves_no_temp_file() {
        let dir = tmp("write");
        let path = index_path(&dir, "gearbox", "go");
        write(&sample_index(), &path).unwrap();
        assert_eq!(read(&path).unwrap(), sample_index());
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec!["gearbox.go.json"]);
        // Overwriting is the same move.
        write(&sample_index(), &path).unwrap();
        assert_eq!(read(&path).unwrap(), sample_index());
    }

    #[test]
    fn read_reports_a_missing_or_unreadable_file() {
        let dir = tmp("read");
        let missing = read(&dir.join("nope.json")).unwrap_err();
        assert!(missing.contains("nope.json"), "{missing}");
        std::fs::write(dir.join("bad.json"), "{not json").unwrap();
        let bad = read(&dir.join("bad.json")).unwrap_err();
        assert!(bad.contains("bad.json"), "{bad}");
    }

    #[test]
    fn write_reports_a_parent_that_cannot_be_created() {
        let dir = tmp("nodir");
        std::fs::write(dir.join("file"), "x").unwrap();
        let err = write(&sample_index(), &dir.join("file").join("index.json")).unwrap_err();
        assert!(err.contains("file"), "{err}");
    }
}
