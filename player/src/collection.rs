//! A replay collection on disk: collection.toml, replays/, blobs/ (SPEC.md sections 1, 2, 4, 8).

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
        let cfg_path = root.join("collection.toml");
        let text = std::fs::read_to_string(&cfg_path).with_context(|| format!("reading {}", cfg_path.display()))?;
        let config: Config = toml::from_str(&text).with_context(|| format!("parsing {}", cfg_path.display()))?;
        if config.format != 1 {
            bail!("collection format {} is not supported", config.format);
        }
        let mut replays = BTreeMap::new();
        let mut paths = HashMap::new();
        let dir = root.join("replays");
        if dir.is_dir() {
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.extension().and_then(|e| e.to_str()) != Some("replay") {
                    continue;
                }
                let r = Replay::load(&path)?;
                let id = r.id()?.to_string();
                if path.file_stem().and_then(|s| s.to_str()) != Some(id.as_str()) {
                    bail!("{} has id {id}; the file must be named {id}.replay", path.display());
                }
                paths.insert(id.clone(), path);
                replays.insert(id, r);
            }
        }
        let c = Collection { root: root.to_path_buf(), config, replays, paths };
        c.check_tree()?;
        Ok(c)
    }

    fn check_tree(&self) -> Result<()> {
        for (id, r) in &self.replays {
            if let Some(p) = r.parent() {
                if !self.replays.contains_key(p) {
                    bail!("replay {id} names parent {p}, which is not in replays/");
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
        self.root.join(".emucov")
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
            std::fs::write(gi, ".emucov/\n")?;
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
        let path = self.paths.get(id).cloned().unwrap_or_else(|| self.root.join("replays").join(format!("{id}.replay")));
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, self.replays[id].to_text())?;
        self.paths.insert(id.to_string(), path);
        Ok(())
    }

    pub fn blob_path(&self, blob: &str) -> PathBuf {
        self.root.join("blobs").join(blob)
    }

    pub fn read_blob(&self, blob: &str) -> Result<Vec<u8>> {
        let data = std::fs::read(self.blob_path(blob)).with_context(|| format!("reading blob {blob}"))?;
        let actual = blob_id(&data);
        if actual != blob {
            bail!("blob {blob} has the wrong content (hashes to {actual})");
        }
        Ok(data)
    }

    #[allow(dead_code)]
    pub fn write_blob(&self, data: &[u8]) -> Result<String> {
        let id = blob_id(data);
        std::fs::create_dir_all(self.root.join("blobs"))?;
        std::fs::write(self.blob_path(&id), data)?;
        Ok(id)
    }
}

pub fn blob_id(data: &[u8]) -> String {
    format!("blake3-{}", blake3::hash(data).to_hex())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    use sha2::Digest;
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(format!("{:x}", sha2::Sha256::digest(&data)))
}
