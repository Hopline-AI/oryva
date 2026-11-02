//! Copies freshly emitted files into an output directory without clobbering anything the user
//! owns. A manifest in `.oryva/` records what oryva wrote and the hash it wrote.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;

const MANIFEST: &str = ".oryva/manifest.json";
pub const IR_FILE: &str = ".oryva/ir.json";

#[derive(Serialize, Deserialize, Default)]
struct Manifest {
    oryva_version: String,
    ir_version: u32,
    files: BTreeMap<String, String>,
}

/// A checked, not yet applied sync of one staging dir into one output dir.
pub struct Sync {
    out: PathBuf,
    new: BTreeMap<String, Vec<u8>>,
    stale: Vec<String>,
    ir_json: String,
    ir_version: u32,
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn walk(root: &Path, dir: &Path, into: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            walk(root, &path, into)?;
        } else {
            let rel = path.strip_prefix(root)?;
            let rel: Vec<_> = rel.iter().map(|c| c.to_string_lossy()).collect();
            into.insert(rel.join("/"), std::fs::read(&path)?);
        }
    }
    Ok(())
}

/// Manifest paths come from a file the user can edit; refuse anything that leaves `out`.
fn is_safe(rel: &str) -> bool {
    Path::new(rel)
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
}

impl Sync {
    /// Compares `stage` with `out` and fails, before writing anything, if a file would be lost.
    pub fn check(stage: &Path, out: &Path, ir: &oryva_ir::Api, force: bool) -> Result<Sync> {
        let mut new = BTreeMap::new();
        walk(stage, stage, &mut new)?;

        let manifest_path = out.join(MANIFEST);
        let old: Manifest = match std::fs::read_to_string(&manifest_path) {
            Ok(s) => {
                serde_json::from_str(&s).map_err(|e| format!("{}: {e}", manifest_path.display()))?
            }
            Err(_) => Manifest::default(),
        };
        if let Some(bad) = old.files.keys().find(|k| !is_safe(k)) {
            return Err(format!("{}: unsafe path `{bad}`", manifest_path.display()).into());
        }

        let edited: Vec<&String> = old
            .files
            .iter()
            .filter(|(rel, h)| std::fs::read(out.join(rel)).is_ok_and(|b| hash(&b) != **h))
            .map(|(rel, _)| rel)
            .collect();
        let collisions: Vec<&String> = new
            .keys()
            .filter(|rel| !old.files.contains_key(*rel) && out.join(rel).exists())
            .collect();

        let mut problems = String::new();
        if !collisions.is_empty() {
            problems += &format!(
                "{} already has files oryva did not generate, at paths it needs:\n{}\nMove or delete them; oryva never touches files it didn't generate.\n",
                out.display(),
                list(&collisions)
            );
        }
        if !edited.is_empty() && !force {
            problems += &format!(
                "generated files in {} were edited by hand:\n{}\nMove custom code into files oryva doesn't generate, or rerun with --force to overwrite your edits.\n",
                out.display(),
                list(&edited)
            );
        }
        if !problems.is_empty() {
            return Err(problems.trim_end().into());
        }

        let stale = old
            .files
            .keys()
            .filter(|k| !new.contains_key(*k))
            .cloned()
            .collect();
        Ok(Sync {
            out: out.to_path_buf(),
            new,
            stale,
            ir_json: serde_json::to_string_pretty(ir)?,
            ir_version: ir.ir_version,
        })
    }

    pub fn file_count(&self) -> usize {
        self.new.len()
    }

    pub fn stale_count(&self) -> usize {
        self.stale.len()
    }

    /// Writes the new files, deletes stale ones, then writes the manifest.
    pub fn apply(self) -> Result<()> {
        for (rel, bytes) in &self.new {
            let path = self.out.join(rel);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, bytes)?;
        }
        for rel in &self.stale {
            let path = self.out.join(rel);
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            // remove_dir only succeeds on an empty directory, which is the point.
            let mut dir = path.parent();
            while let Some(d) = dir.filter(|d| *d != self.out) {
                if std::fs::remove_dir(d).is_err() {
                    break;
                }
                dir = d.parent();
            }
        }
        std::fs::create_dir_all(self.out.join(".oryva"))?;
        std::fs::write(self.out.join(IR_FILE), &self.ir_json)?;
        let manifest = Manifest {
            oryva_version: env!("CARGO_PKG_VERSION").into(),
            ir_version: self.ir_version,
            files: self.new.iter().map(|(k, v)| (k.clone(), hash(v))).collect(),
        };
        std::fs::write(
            self.out.join(MANIFEST),
            serde_json::to_string_pretty(&manifest)?,
        )?;
        Ok(())
    }
}

fn list(items: &[&String]) -> String {
    items
        .iter()
        .map(|i| format!("  {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}
