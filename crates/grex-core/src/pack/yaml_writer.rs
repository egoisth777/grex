//! Append child entries to `pack.yaml`.
//!
//! Bridge between the audit log (`.grex/events.jsonl`) and the
//! declarative manifest (`.grex/pack.yaml`) that the walker reads. Used
//! by [`crate::add::add_pack`] and [`crate::import::import_from_repos_json`]
//! so freshly-registered packs become visible to `grex ls`, `grex sync`,
//! and `grex status` without requiring a hand-edit of `pack.yaml`.
//!
//! # Why a raw `serde_yaml::Value` round-trip
//!
//! `PackManifest` is the typed view, but serializing it back is lossy:
//! actions are key-dispatched (not `#[serde(untagged)]`) and the
//! parse/serialize surface diverges across action variants. A
//! `Value`-level round-trip preserves every field grex does not yet
//! understand — including future `x-*` extensions and hand-authored
//! comments-as-keys — at the cost of dropping standalone YAML comments
//! (a serde_yaml 0.9 limitation). The pack-spec promises that
//! `children:` is grex-owned, so authors who annotate that section with
//! comments have already opted into round-trip loss.
//!
//! # Idempotence
//!
//! [`append_child_to_pack_yaml`] returns
//! [`AppendOutcome::AlreadyPresent`] when the target effective path is
//! already in the sequence. Callers can fold this into a "skip"
//! statistic without re-querying the file.

use std::fs;
use std::path::{Path, PathBuf};

use serde_yaml::{Mapping, Value};
use thiserror::Error;

use crate::pack::SUPPORTED_SCHEMA_VERSION;

/// Lightweight projection of [`crate::pack::ChildRef`] used by the
/// writer.
///
/// `path` is required (we always know the resolved path at the time
/// `add` / `import` calls the writer). `git_ref` is optional and lands
/// as a `ref:` key when set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildEntry {
    pub url: String,
    pub path: String,
    pub git_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendOutcome {
    /// The child entry was newly appended to the sequence.
    Appended,
    /// A child entry with the same `path` was already present;
    /// `pack.yaml` was not modified.
    AlreadyPresent,
}

#[derive(Debug, Error)]
pub enum PackYamlWriteError {
    #[error("cannot read pack manifest {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot write pack manifest {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot create directory {path}: {source}")]
    Mkdir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed pack manifest at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("pack manifest at {path}: top-level YAML must be a mapping")]
    NotMapping { path: PathBuf },
    #[error("pack manifest at {path}: `children:` exists but is not a sequence")]
    ChildrenNotSequence { path: PathBuf },
    #[error("emit pack manifest at {path}: {source}")]
    Emit {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
}

/// Append `entry` to `pack.yaml.children`. Creates the file (with the
/// minimal manifest skeleton) when it does not yet exist.
///
/// Returns [`AppendOutcome::AlreadyPresent`] when an entry with the same
/// `path` is already present in the sequence; the file is then left
/// untouched.
pub fn append_child_to_pack_yaml(
    pack_yaml_path: &Path,
    entry: &ChildEntry,
) -> Result<AppendOutcome, PackYamlWriteError> {
    let mut root = if pack_yaml_path.exists() {
        load_mapping(pack_yaml_path)?
    } else {
        let skeleton = minimal_skeleton(pack_yaml_path);
        ensure_parent_dir(pack_yaml_path)?;
        skeleton
    };

    let children_key = Value::String("children".to_string());
    let already = child_already_present(&root, &children_key, &entry.path, pack_yaml_path)?;
    if already {
        return Ok(AppendOutcome::AlreadyPresent);
    }

    insert_child(&mut root, children_key, entry)?;
    write_mapping(pack_yaml_path, &root)?;
    Ok(AppendOutcome::Appended)
}

fn load_mapping(pack_yaml_path: &Path) -> Result<Mapping, PackYamlWriteError> {
    let raw = fs::read_to_string(pack_yaml_path).map_err(|source| PackYamlWriteError::Read {
        path: pack_yaml_path.to_path_buf(),
        source,
    })?;
    let value: Value = if raw.trim().is_empty() {
        Value::Mapping(Mapping::new())
    } else {
        serde_yaml::from_str(&raw).map_err(|source| PackYamlWriteError::Parse {
            path: pack_yaml_path.to_path_buf(),
            source,
        })?
    };
    match value {
        Value::Mapping(m) => Ok(m),
        Value::Null => Ok(Mapping::new()),
        _ => Err(PackYamlWriteError::NotMapping { path: pack_yaml_path.to_path_buf() }),
    }
}

fn minimal_skeleton(pack_yaml_path: &Path) -> Mapping {
    let mut m = Mapping::new();
    m.insert(
        Value::String("schema_version".into()),
        Value::String(SUPPORTED_SCHEMA_VERSION.into()),
    );
    m.insert(Value::String("name".into()), Value::String(derive_pack_name(pack_yaml_path)));
    m.insert(Value::String("type".into()), Value::String("meta".into()));
    m.insert(Value::String("actions".into()), Value::Sequence(Vec::new()));
    m.insert(Value::String("children".into()), Value::Sequence(Vec::new()));
    m
}

fn derive_pack_name(pack_yaml_path: &Path) -> String {
    pack_yaml_path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .map(sanitize_pack_name)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "workspace".to_string())
}

fn sanitize_pack_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_hyphen = false;
    for c in raw.chars() {
        let lower = c.to_ascii_lowercase();
        let ok = matches!(lower, 'a'..='z' | '0'..='9' | '-');
        if ok {
            out.push(lower);
            last_hyphen = lower == '-';
        } else if !last_hyphen && !out.is_empty() {
            out.push('-');
            last_hyphen = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    while let Some(first) = out.chars().next() {
        if first.is_ascii_alphabetic() {
            break;
        }
        out.remove(0);
    }
    out
}

fn ensure_parent_dir(pack_yaml_path: &Path) -> Result<(), PackYamlWriteError> {
    if let Some(parent) = pack_yaml_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|source| PackYamlWriteError::Mkdir { path: parent.to_path_buf(), source })?;
    }
    Ok(())
}

fn child_already_present(
    root: &Mapping,
    children_key: &Value,
    candidate_path: &str,
    pack_yaml_path: &Path,
) -> Result<bool, PackYamlWriteError> {
    let Some(seq_value) = root.get(children_key) else {
        return Ok(false);
    };
    match seq_value {
        Value::Sequence(items) => {
            Ok(items.iter().any(|c| child_path(c).as_deref() == Some(candidate_path)))
        }
        Value::Null => Ok(false),
        _ => Err(PackYamlWriteError::ChildrenNotSequence { path: pack_yaml_path.to_path_buf() }),
    }
}

fn child_path(value: &Value) -> Option<String> {
    let Value::Mapping(m) = value else {
        return None;
    };
    match m.get(Value::String("path".into())) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => None,
        None => match m.get(Value::String("url".into())) {
            Some(Value::String(url)) => Some(infer_path(url)),
            _ => None,
        },
    }
}

fn infer_path(url: &str) -> String {
    let trimmed = url.trim_end_matches(['/', '\\']);
    let tail = trimmed.rsplit(['/', '\\', ':']).next().unwrap_or(trimmed);
    tail.strip_suffix(".git").unwrap_or(tail).to_string()
}

fn insert_child(
    root: &mut Mapping,
    children_key: Value,
    entry: &ChildEntry,
) -> Result<(), PackYamlWriteError> {
    let mut child_map = Mapping::new();
    child_map.insert(Value::String("url".into()), Value::String(entry.url.clone()));
    child_map.insert(Value::String("path".into()), Value::String(entry.path.clone()));
    if let Some(r) = &entry.git_ref {
        child_map.insert(Value::String("ref".into()), Value::String(r.clone()));
    }

    let existing = root.remove(&children_key);
    let mut seq = match existing {
        Some(Value::Sequence(items)) => items,
        Some(Value::Null) | None => Vec::new(),
        Some(_) => {
            return Err(PackYamlWriteError::ChildrenNotSequence { path: PathBuf::new() });
        }
    };
    seq.push(Value::Mapping(child_map));
    root.insert(children_key, Value::Sequence(seq));
    Ok(())
}

fn write_mapping(pack_yaml_path: &Path, mapping: &Mapping) -> Result<(), PackYamlWriteError> {
    let body = serde_yaml::to_string(&Value::Mapping(mapping.clone())).map_err(|source| {
        PackYamlWriteError::Emit { path: pack_yaml_path.to_path_buf(), source }
    })?;
    fs::write(pack_yaml_path, body)
        .map_err(|source| PackYamlWriteError::Write { path: pack_yaml_path.to_path_buf(), source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn read(p: &Path) -> String {
        fs::read_to_string(p).unwrap()
    }

    #[test]
    fn creates_pack_yaml_when_missing() {
        let dir = tempdir().unwrap();
        let pack = dir.path().join("ws/.grex/pack.yaml");
        let outcome = append_child_to_pack_yaml(
            &pack,
            &ChildEntry { url: "u".into(), path: "foo".into(), git_ref: None },
        )
        .unwrap();
        assert_eq!(outcome, AppendOutcome::Appended);
        assert!(pack.exists());
        let body = read(&pack);
        assert!(body.contains("schema_version"));
        assert!(body.contains("name: ws"));
        assert!(body.contains("type: meta"));
        assert!(body.contains("children:"));
        assert!(body.contains("path: foo"));
        assert!(body.contains("url: u"));
    }

    #[test]
    fn appends_to_existing_children_sequence() {
        let dir = tempdir().unwrap();
        let pack = dir.path().join("pack.yaml");
        fs::write(
            &pack,
            "schema_version: \"1\"\nname: ws\ntype: meta\nchildren:\n  - url: a\n    path: a\n",
        )
        .unwrap();

        let outcome = append_child_to_pack_yaml(
            &pack,
            &ChildEntry { url: "b".into(), path: "b".into(), git_ref: None },
        )
        .unwrap();
        assert_eq!(outcome, AppendOutcome::Appended);
        let body = read(&pack);
        assert!(body.contains("path: a"));
        assert!(body.contains("path: b"));
    }

    #[test]
    fn idempotent_on_duplicate_path() {
        let dir = tempdir().unwrap();
        let pack = dir.path().join("pack.yaml");
        fs::write(
            &pack,
            "schema_version: \"1\"\nname: ws\ntype: meta\nchildren:\n  - url: a\n    path: a\n",
        )
        .unwrap();
        let before = read(&pack);

        let outcome = append_child_to_pack_yaml(
            &pack,
            &ChildEntry { url: "other".into(), path: "a".into(), git_ref: None },
        )
        .unwrap();
        assert_eq!(outcome, AppendOutcome::AlreadyPresent);
        assert_eq!(read(&pack), before, "duplicate must not mutate file");
    }

    #[test]
    fn preserves_existing_actions_and_x_extensions() {
        let dir = tempdir().unwrap();
        let pack = dir.path().join("pack.yaml");
        fs::write(
            &pack,
            "schema_version: \"1\"\nname: ws\ntype: meta\nactions:\n  - mkdir:\n      path: ./out\nx-custom: keep-me\nchildren: []\n",
        )
        .unwrap();

        let outcome = append_child_to_pack_yaml(
            &pack,
            &ChildEntry { url: "u".into(), path: "p".into(), git_ref: None },
        )
        .unwrap();
        assert_eq!(outcome, AppendOutcome::Appended);
        let body = read(&pack);
        assert!(body.contains("mkdir"));
        assert!(body.contains("x-custom"));
        assert!(body.contains("path: p"));
    }

    #[test]
    fn ref_round_trips_when_provided() {
        let dir = tempdir().unwrap();
        let pack = dir.path().join("pack.yaml");
        append_child_to_pack_yaml(
            &pack,
            &ChildEntry { url: "u".into(), path: "p".into(), git_ref: Some("dev".into()) },
        )
        .unwrap();
        let body = read(&pack);
        assert!(body.contains("ref: dev"));
    }

    #[test]
    fn null_children_treated_as_empty_sequence() {
        let dir = tempdir().unwrap();
        let pack = dir.path().join("pack.yaml");
        fs::write(&pack, "schema_version: \"1\"\nname: ws\ntype: meta\nchildren: ~\n").unwrap();
        let outcome = append_child_to_pack_yaml(
            &pack,
            &ChildEntry { url: "u".into(), path: "p".into(), git_ref: None },
        )
        .unwrap();
        assert_eq!(outcome, AppendOutcome::Appended);
        assert!(read(&pack).contains("path: p"));
    }

    #[test]
    fn rejects_when_children_not_sequence() {
        let dir = tempdir().unwrap();
        let pack = dir.path().join("pack.yaml");
        fs::write(&pack, "schema_version: \"1\"\nname: ws\ntype: meta\nchildren: oops\n").unwrap();
        let err = append_child_to_pack_yaml(
            &pack,
            &ChildEntry { url: "u".into(), path: "p".into(), git_ref: None },
        )
        .unwrap_err();
        assert!(matches!(err, PackYamlWriteError::ChildrenNotSequence { .. }));
    }

    #[test]
    fn sanitize_pack_name_handles_underscores_and_dots() {
        assert_eq!(sanitize_pack_name("My_Pack.v1"), "my-pack-v1");
        assert_eq!(sanitize_pack_name("123abc"), "abc");
        assert_eq!(sanitize_pack_name("ALL_CAPS"), "all-caps");
        assert_eq!(sanitize_pack_name("..."), "");
    }
}
