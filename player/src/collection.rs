//! A project on disk: config.toml, segments/, blobs/ (SPEC.md sections 1, 2, 4, 8).

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::format::Replay;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    pub format: u32,
    pub name: String,
    pub rom: RomConfig,
    pub core: CoreConfig,
    #[serde(default)]
    pub ports: Vec<PortConfig>,
    pub hash: HashConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick: Option<TickConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<SymbolsConfig>,
    /// Thresholds for the local database (CACHE_PLAN.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<crate::cache::CacheConfig>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RomConfig {
    pub sha256: String,
    #[serde(default)]
    pub file: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CoreConfig {
    pub name: String,
    pub build: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PortConfig {
    pub port: u32,
    pub device: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HashConfig {
    pub algorithm: String,
    pub regions: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TickConfig {
    #[serde(default)]
    pub exec: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SymbolsConfig {
    pub file: String,
    pub format: String,
    /// Bytes before PRG data in the ROM file (16 for iNES).
    #[serde(default)]
    pub rom_header: Option<u64>,
}

/// Local, uncommitted settings: where the ROM and core live on this machine.
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct LocalConfig {
    #[serde(default)]
    pub rom: Option<PathBuf>,
    #[serde(default)]
    pub core: Option<PathBuf>,
    /// false = run without the local database, as the tools did before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<bool>,
    /// The GUI's in-memory snapshot cache limit, in MB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_memory_mb: Option<u64>,
    /// Overrides config.toml's [cache] on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<crate::cache::CacheConfig>,
}

pub struct Collection {
    pub root: PathBuf,
    pub config: Config,
    pub replays: BTreeMap<String, Replay>,
    pub paths: HashMap<String, PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Unverified,
    Clean,
    Dirty,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Unverified => "unverified",
            Status::Clean => "clean",
            Status::Dirty => "dirty",
        }
    }
}

impl Collection {
    pub fn open(root: &Path) -> Result<Collection> {
        // The local folder used to be `.emucov`; move it to `.local` the first time.
        let (old, new) = (root.join(".emucov"), root.join(".local"));
        if old.is_dir() && !new.exists() {
            std::fs::rename(&old, &new).with_context(|| format!("moving {} to {}", old.display(), new.display()))?;
            // Keep it ignored: a collection's .gitignore that names .emucov/ gets .local/ instead.
            let gi = root.join(".gitignore");
            if let Ok(text) = std::fs::read_to_string(&gi) {
                if text.lines().any(|l| l.trim() == ".emucov/" || l.trim() == ".emucov") && !text.lines().any(|l| l.trim().trim_end_matches('/') == ".local") {
                    let fixed: Vec<String> = text.lines().map(|l| if l.trim().trim_end_matches('/') == ".emucov" { ".local/".to_string() } else { l.to_string() }).collect();
                    std::fs::write(&gi, fixed.join("\n") + "\n")?;
                }
            }
        }
        // Before the layout change: collection.toml and replays/<id>.replay.
        let (old_cfg, cfg_path) = (root.join("collection.toml"), root.join("config.toml"));
        if old_cfg.is_file() && !cfg_path.exists() {
            std::fs::rename(&old_cfg, &cfg_path).with_context(|| format!("moving {} to {}", old_cfg.display(), cfg_path.display()))?;
        }
        let (old_dir, dir) = (root.join("replays"), root.join("segments"));
        if old_dir.is_dir() && !dir.exists() {
            std::fs::rename(&old_dir, &dir).with_context(|| format!("moving {} to {}", old_dir.display(), dir.display()))?;
        }
        if dir.is_dir() {
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.extension().and_then(|e| e.to_str()) == Some("replay") {
                    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
                    let new = path.with_extension(SEGMENT_EXT);
                    std::fs::write(&new, crate::format::upgrade_text(&text))?;
                    std::fs::remove_file(&path)?;
                }
            }
        }
        let text = std::fs::read_to_string(&cfg_path).with_context(|| format!("reading {}", cfg_path.display()))?;
        let config: Config = toml::from_str(&text).with_context(|| format!("parsing {}", cfg_path.display()))?;
        if config.format != 1 {
            bail!("collection format {} is not supported", config.format);
        }
        let mut replays = BTreeMap::new();
        let mut paths = HashMap::new();
        if dir.is_dir() {
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.extension().and_then(|e| e.to_str()) != Some(SEGMENT_EXT) {
                    continue;
                }
                let r = Replay::load(&path)?;
                let id = r.id()?.to_string();
                if path.file_stem().and_then(|s| s.to_str()) != Some(id.as_str()) {
                    bail!("{} has id {id}; the file must be named {id}.{SEGMENT_EXT}", path.display());
                }
                paths.insert(id.clone(), path);
                replays.insert(id, r);
            }
        }
        let c = Collection { root: root.to_path_buf(), config, replays, paths };
        c.check_tree()?;
        c.upgrade_blob_names()?;
        Ok(c)
    }

    fn check_tree(&self) -> Result<()> {
        for (id, r) in &self.replays {
            if let Some(p) = r.parent() {
                if !self.replays.contains_key(p) {
                    bail!("replay {id} names parent {p}, which is not in segments/");
                }
            }
            // Cycle check.
            let mut seen = vec![id.as_str()];
            let mut cur = r.parent();
            while let Some(p) = cur {
                if seen.contains(&p) {
                    bail!("replay {id} has a parent loop");
                }
                seen.push(p);
                cur = self.replays[p].parent();
            }
            if r.parent().is_some() && (r.get("save").is_some() || r.get("sram").is_some()) {
                bail!("replay {id} has a parent, so it cannot also name `save` or `sram` (only trunks start from blobs)");
            }
        }
        Ok(())
    }

    pub fn local_dir(&self) -> PathBuf {
        self.root.join(".local")
    }

    pub fn load_local(&self) -> Result<LocalConfig> {
        let p = self.local_dir().join("local.toml");
        if !p.exists() {
            return Ok(LocalConfig::default());
        }
        Ok(toml::from_str(&std::fs::read_to_string(&p)?)?)
    }

    pub fn save_local(&self, local: &LocalConfig) -> Result<()> {
        std::fs::create_dir_all(self.local_dir())?;
        std::fs::write(self.local_dir().join("local.toml"), toml::to_string_pretty(local)?)?;
        let gi = self.root.join(".gitignore");
        if !gi.exists() {
            std::fs::write(gi, ".local/\n")?;
        }
        Ok(())
    }

    pub fn children(&self, id: Option<&str>) -> Vec<&str> {
        let mut v: Vec<&str> = self.replays.iter().filter(|(_, r)| r.parent() == id).map(|(k, _)| k.as_str()).collect();
        v.sort_by_key(|k| (self.replays[*k].title().to_string(), k.to_string()));
        v
    }

    /// Replays from the trunk down to `id`, inclusive.
    pub fn chain(&self, id: &str) -> Result<Vec<&str>> {
        let mut out = vec![];
        let mut cur = Some(self.resolve(id)?);
        while let Some(c) = cur {
            out.push(c);
            cur = self.replays[c].parent();
        }
        out.reverse();
        Ok(out)
    }

    /// Accepts a full id or a unique prefix.
    pub fn resolve<'a>(&'a self, id: &str) -> Result<&'a str> {
        if let Some((k, _)) = self.replays.get_key_value(id) {
            return Ok(k.as_str());
        }
        let m: Vec<&str> = self.replays.keys().filter(|k| k.starts_with(id)).map(|k| k.as_str()).collect();
        match m.len() {
            1 => Ok(m[0]),
            0 => Err(anyhow!("no replay with id {id}")),
            _ => Err(anyhow!("id prefix {id} matches {} replays", m.len())),
        }
    }

    /// Depth-first order, trunks first.
    pub fn tree_order(&self) -> Vec<(&str, usize)> {
        let mut out = vec![];
        fn walk<'a>(c: &'a Collection, id: Option<&str>, depth: usize, out: &mut Vec<(&'a str, usize)>) {
            for ch in c.children(id) {
                out.push((ch, depth));
                walk(c, Some(ch), depth + 1, out);
            }
        }
        walk(self, None, 0, &mut out);
        out
    }

    /// Status worked out from hashes alone (SPEC.md section 8).
    pub fn status(&self, id: &str) -> Status {
        let r = &self.replays[id];
        if let (Some(p), Some(start)) = (r.parent(), r.get("start-hash")) {
            match self.replays[p].get("end-hash") {
                Some(pe) if pe != start => return Status::Dirty,
                None => return Status::Dirty,
                _ => {}
            }
        }
        if r.get("end-hash").is_none() {
            Status::Unverified
        } else {
            Status::Clean
        }
    }

    /// Like `status`, but a replay below a dirty one is dirty too: its start
    /// state can no longer be trusted until the branch above is re-verified.
    pub fn tree_status(&self, id: &str) -> Status {
        let s = self.status(id);
        if s == Status::Dirty {
            return s;
        }
        match self.replays[id].parent() {
            Some(p) if self.tree_status(p) == Status::Dirty => Status::Dirty,
            _ => s,
        }
    }

    pub fn save_replay(&mut self, id: &str) -> Result<()> {
        let path = self.paths.get(id).cloned().unwrap_or_else(|| self.root.join("segments").join(format!("{id}.{SEGMENT_EXT}")));
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, self.replays[id].to_text())?;
        self.paths.insert(id.to_string(), path);
        Ok(())
    }

    /// Deletes the file of a replay that is no longer in the collection (after an undo of
    /// a split or of a new segment). Does nothing if the replay was never saved.
    pub fn delete_replay_file(&mut self, id: &str) -> Result<()> {
        if let Some(path) = self.paths.remove(id) {
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
        }
        Ok(())
    }

    /// `blobs/<hash>.srm` for the header field `sram`, `blobs/<hash>.state` for `save`.
    pub fn blob_path(&self, blob: &str, field: &str) -> PathBuf {
        self.root.join("blobs").join(format!("{blob}.{}", blob_ext(field)))
    }

    /// Blobs used to be named `blake3-<64 hex>` with no extension. Renames the ones the
    /// segments name to the current form.
    fn upgrade_blob_names(&self) -> Result<()> {
        for r in self.replays.values() {
            for field in ["save", "sram"] {
                let Some(blob) = r.get(field) else { continue };
                let path = self.blob_path(blob, field);
                if path.exists() {
                    continue;
                }
                let Ok(entries) = std::fs::read_dir(self.root.join("blobs")) else { continue };
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.starts_with("blake3-") && crate::format::normalize_hash(&name) == blob {
                        std::fs::rename(e.path(), &path).with_context(|| format!("renaming blob {name}"))?;
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn read_blob(&self, blob: &str, field: &str) -> Result<Vec<u8>> {
        let data = std::fs::read(self.blob_path(blob, field)).with_context(|| format!("reading blob {blob}"))?;
        let actual = blob_id(&data);
        if actual != blob {
            bail!("blob {blob} has the wrong content (hashes to {actual})");
        }
        Ok(data)
    }

    #[allow(dead_code)]
    pub fn write_blob(&self, data: &[u8], field: &str) -> Result<String> {
        let id = blob_id(data);
        std::fs::create_dir_all(self.root.join("blobs"))?;
        std::fs::write(self.blob_path(&id, field), data)?;
        Ok(id)
    }
}

pub fn blob_id(data: &[u8]) -> String {
    crate::format::hash_hex(&blake3::hash(data))
}

/// The extension of a segment file.
pub const SEGMENT_EXT: &str = "segment";

/// A blob's extension, from the header field that names it.
pub fn blob_ext(field: &str) -> &'static str {
    if field == "sram" { "srm" } else { "state" }
}

pub fn sha256_file(path: &Path) -> Result<String> {
    use sha2::Digest;
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(format!("{:x}", sha2::Sha256::digest(&data)))
}
