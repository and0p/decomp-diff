//! Plays replays on a loaded core (SPEC.md sections 5 and 6).

use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::collection::{sha256_file, Collection};
use crate::format::{device_from_name, Command, InputKey, Replay};
use crate::libretro::{region_memory_id, Core, CoverageRegion, Frame};

pub struct ReplayResult {
    pub id: String,
    pub start_frame: u64,
    pub frames: u64,
    pub start_hash: String,
    pub end_hash: String,
    pub checkpoints: Vec<(u64, String)>,
    pub ticks: Option<u64>,
    pub poked: bool,
    pub coverage: Vec<CoverageRegion>,
    pub screenshots: Vec<(u64, Frame)>,
    pub end_state: Vec<u8>,
}

#[derive(Default, Clone)]
pub struct PlayOptions {
    /// Capture the video frame of the last frame of each replay.
    pub end_screenshot: bool,
    /// Extra frames (relative) to capture, besides [SCREENSHOT] lines.
    pub extra_shots: Vec<u64>,
}

#[allow(dead_code)]
pub struct Session<'c> {
    pub col: &'c Collection,
    pub core: Core,
    boot_state: Vec<u8>,
    pub setup_id: String,
    pub rom_path: PathBuf,
}

impl<'c> Session<'c> {
    pub fn open(col: &'c Collection, rom: &Path, core_path: &Path) -> Result<Session<'c>> {
        let rom_sha = sha256_file(rom)?;
        if !col.config.rom.sha256.is_empty() && rom_sha != col.config.rom.sha256 {
            bail!(
                "ROM {} has sha256 {rom_sha}, but the collection is pinned to {}",
                rom.display(),
                col.config.rom.sha256
            );
        }
        let core_sha = sha256_file(core_path)?;
        if !col.config.core.sha256.is_empty() && core_sha != col.config.core.sha256 {
            bail!("core {} has sha256 {core_sha}, but the collection is pinned to {}", core_path.display(), col.config.core.sha256);
        }
        let work = col.local_dir();
        let mut core = Core::load(core_path, &work, &col.config.core.options)?;
        core.load_game(rom)?;

        if let Some(t) = &col.config.tick {
            if let Some(exec) = &t.exec {
                let (region, off) = parse_region_offset(exec)?;
                match core.coverage_region_index(&region) {
                    Some(i) => core.set_tick_exec(Some(i), off),
                    None => eprintln!("warning: tick region {region} is not reported by this core's connector; ticks are off"),
                }
            }
        }

        let boot_state = core.serialize().context("saving the power-on state")?;
        let mut h = blake3::Hasher::new();
        h.update(col.config.core.build.as_bytes());
        h.update(core_sha.as_bytes());
        h.update(rom_sha.as_bytes());
        for (k, v) in core.effective_options() {
            h.update(k.as_bytes());
            h.update(v.as_bytes());
        }
        let setup_id = h.finalize().to_hex()[..16].to_string();
        Ok(Session { col, core, boot_state, setup_id, rom_path: rom.to_path_buf() })
    }

    pub fn state_cache_dir(&self) -> PathBuf {
        self.col.local_dir().join("states").join(&self.setup_id)
    }

    /// Ports' devices in effect at the start of `id`: collection defaults plus ancestors' [DEVICE] lines.
    fn devices_at_start(&self, id: &str) -> Result<BTreeMap<u32, u32>> {
        let mut devs = BTreeMap::new();
        for p in &self.col.config.ports {
            if p.port == 0 {
                bail!("collection.toml ports start at 1");
            }
            devs.insert(p.port - 1, device_from_name(&p.device)?);
        }
        let chain = self.col.chain(id)?;
        for anc in &chain[..chain.len() - 1] {
            for l in &self.col.replays[*anc].lines {
                if let Command::Device { port, device } = l.command {
                    devs.insert(port, device);
                }
            }
        }
        Ok(devs)
    }

    pub fn state_hash(&mut self) -> Result<String> {
        let mut h = blake3::Hasher::new();
        for region in self.col.config.hash.regions.clone() {
            let bytes: Vec<u8> = if let Some(mid) = region_memory_id(&region) {
                self.core.memory(mid).map(|m| m.to_vec()).unwrap_or_default()
            } else if let Some(range) = region.strip_prefix("BUS:") {
                let (a, b) = range.split_once('-').ok_or_else(|| anyhow!("BUS hash ranges look like BUS:0x0000-0x07FF"))?;
                let (a, b) = (parse_hex(a)? as usize, parse_hex(b)? as usize);
                (a..=b).map(|addr| self.core.bus_byte(addr).map(|p| unsafe { *p }).unwrap_or(0)).collect()
            } else {
                bail!("unknown hash region {region}");
            };
            h.update(region.as_bytes());
            h.update(&[0]);
            h.update(&(bytes.len() as u64).to_le_bytes());
            h.update(&bytes);
        }
        Ok(format!("blake3-{}", h.finalize().to_hex()))
    }

    /// Puts the core at the start of trunk `r`.
    pub fn start_trunk(&mut self, r: &Replay) -> Result<()> {
        let boot = self.boot_state.clone();
        self.core.unserialize(&boot)?;
        if let Some(save) = r.get("save") {
            let data = self.col.read_blob(save)?;
            self.core.unserialize(&data).with_context(|| format!("loading save blob {save}"))?;
        }
        if let Some(sram) = r.get("sram") {
            let data = self.col.read_blob(sram)?;
            let mem = self.core.memory(crate::libretro::MEMORY_SAVE_RAM).ok_or_else(|| anyhow!("core exposes no save RAM"))?;
            if data.len() != mem.len() {
                eprintln!("warning: SRAM blob is {} bytes but the core's save RAM is {}", data.len(), mem.len());
            }
            let n = data.len().min(mem.len());
            mem[..n].copy_from_slice(&data[..n]);
        }
        Ok(())
    }

    pub fn load_state(&mut self, state: &[u8]) -> Result<()> {
        self.core.unserialize(state)
    }

    /// Plays one replay from the core's current state.
    pub fn play(&mut self, id: &str, start_frame: u64, opts: &PlayOptions) -> Result<ReplayResult> {
        let r = &self.col.replays[id];
        let frames = r.frames()?;
        for (port, dev) in self.devices_at_start(id)? {
            self.core.set_port_device(port, dev);
        }
        self.core.clear_inputs();
        self.core.coverage_clear();

        let start_hash = self.state_hash()?;
        let wanted_checkpoints: Vec<u64> = r.checkpoints()?.into_iter().map(|(f, _)| f).collect();
        let mut checkpoints = vec![];
        let mut shots_wanted: Vec<u64> = opts.extra_shots.clone();
        for l in &r.lines {
            if matches!(l.command, Command::Screenshot) {
                shots_wanted.push(l.frame);
            }
        }
        if opts.end_screenshot && frames > 0 {
            shots_wanted.push(frames - 1);
        }
        let mut screenshots = vec![];
        let mut li = 0;
        let mut impulses: Vec<InputKey> = vec![];
        for f in 0..frames {
            if wanted_checkpoints.contains(&f) {
                checkpoints.push((f, self.state_hash()?));
            }
            while li < r.lines.len() && r.lines[li].frame == f {
                let line = &r.lines[li];
                li += 1;
                match &line.command {
                    Command::Input { key, value, impulse } => {
                        self.core.set_input(*key, *value);
                        if *impulse {
                            impulses.push(*key);
                        }
                    }
                    Command::Device { port, device } => self.core.set_port_device(*port, *device),
                    Command::Write { region, address, bytes } => self.write(region, *address, bytes).with_context(|| format!("line {}", line.line_no))?,
                    Command::Event { .. } | Command::Screenshot => {}
                    Command::Reset => self.core.reset(),
                    Command::Rng { .. } => bail!("line {}: [RNG] is reserved and not supported by this player yet", line.line_no),
                }
            }
            let capture = shots_wanted.contains(&f);
            if let Some(frame) = self.core.run_frame(capture) {
                screenshots.push((f, frame));
            }
            for k in impulses.drain(..) {
                self.core.set_input(k, 0);
            }
        }
        let end_hash = self.state_hash()?;
        let ticks = if self.col.config.tick.is_some() { self.core.tick_count() } else { None };
        let coverage = self.core.coverage_regions();
        let end_state = self.core.serialize()?;
        Ok(ReplayResult {
            id: id.to_string(),
            start_frame,
            frames,
            start_hash,
            end_hash,
            checkpoints,
            ticks,
            poked: r.is_poked(),
            coverage,
            screenshots,
            end_state,
        })
    }

    fn write(&mut self, region: &str, address: u64, bytes: &[u8]) -> Result<()> {
        if region == "BUS" {
            for (i, b) in bytes.iter().enumerate() {
                let p = self.core.bus_byte(address as usize + i).ok_or_else(|| anyhow!("BUS address {:#x} is not mapped", address as usize + i))?;
                unsafe { *p = *b };
            }
            return Ok(());
        }
        let id = region_memory_id(region).ok_or_else(|| anyhow!("unknown region {region}"))?;
        let mem = self.core.memory(id).ok_or_else(|| anyhow!("core exposes no {region}"))?;
        let a = address as usize;
        if a + bytes.len() > mem.len() {
            bail!("[WRITE] {region} {address:#x} runs past the end ({} bytes)", mem.len());
        }
        mem[a..a + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }

    /// Plays every replay in tree order. `from_scratch` replays each chain from its
    /// trunk instead of reusing parents' end states (a determinism check).
    pub fn play_all(&mut self, opts: &PlayOptions, from_scratch: bool, mut each: impl FnMut(&mut Self, &ReplayResult) -> Result<()>) -> Result<()> {
        let order: Vec<String> = self.col.tree_order().into_iter().map(|(id, _)| id.to_string()).collect();
        let mut end_states: HashMap<String, (Vec<u8>, u64)> = HashMap::new();
        for id in &order {
            let chain: Vec<String> = self.col.chain(id)?.into_iter().map(String::from).collect();
            let result = if from_scratch {
                let mut start_frame = 0;
                let quiet = PlayOptions::default();
                self.start_trunk(&self.col.replays[&chain[0]])?;
                for anc in &chain[..chain.len() - 1] {
                    let res = self.play(anc, start_frame, &quiet)?;
                    start_frame += res.frames;
                }
                self.play(id, start_frame, opts)?
            } else {
                let start_frame = match self.col.replays[id].parent() {
                    None => {
                        self.start_trunk(&self.col.replays[id])?;
                        trunk_start_frame(&self.col.replays[id])
                    }
                    Some(p) => {
                        let (state, sf) = end_states.get(p).ok_or_else(|| anyhow!("internal: no end state for parent {p}"))?.clone();
                        self.load_state(&state)?;
                        sf
                    }
                };
                self.play(id, start_frame, opts)?
            };
            if !self.col.children(Some(id)).is_empty() {
                end_states.insert(id.clone(), (result.end_state.clone(), result.start_frame + result.frames));
            }
            self.save_cached_state(&result)?;
            each(self, &result)?;
        }
        Ok(())
    }

    /// Plays the chain down to `id`, starting from the deepest cached ancestor state when allowed.
    pub fn play_chain(&mut self, id: &str, opts: &PlayOptions, use_cache: bool) -> Result<Vec<ReplayResult>> {
        let chain: Vec<String> = self.col.chain(id)?.into_iter().map(String::from).collect();
        let mut start_at = 0;
        let mut start_frame = trunk_start_frame(&self.col.replays[&chain[0]]);
        let mut loaded = false;
        if use_cache {
            for i in (0..chain.len() - 1).rev() {
                if let Some(state) = self.load_cached_state(&chain[i])? {
                    self.load_state(&state)?;
                    start_at = i + 1;
                    start_frame = chain[..=i].iter().map(|c| self.col.replays[c].frames().unwrap_or(0)).sum::<u64>() + trunk_start_frame(&self.col.replays[&chain[0]]);
                    loaded = true;
                    break;
                }
            }
        }
        if !loaded {
            self.start_trunk(&self.col.replays[&chain[0]])?;
        }
        let mut results = vec![];
        for (i, c) in chain.iter().enumerate().skip(start_at) {
            let o = if i == chain.len() - 1 { opts.clone() } else { PlayOptions::default() };
            let res = self.play(c, start_frame, &o)?;
            start_frame += res.frames;
            self.save_cached_state(&res)?;
            results.push(res);
        }
        Ok(results)
    }

    fn save_cached_state(&self, res: &ReplayResult) -> Result<()> {
        let dir = self.state_cache_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("{}.state", res.id)), &res.end_state)?;
        std::fs::write(dir.join(format!("{}.hash", res.id)), &res.end_hash)?;
        Ok(())
    }

    /// A cached end state is used only if it matches the replay's recorded end-hash.
    fn load_cached_state(&self, id: &str) -> Result<Option<Vec<u8>>> {
        let dir = self.state_cache_dir();
        let (sp, hp) = (dir.join(format!("{id}.state")), dir.join(format!("{id}.hash")));
        if !sp.exists() || !hp.exists() {
            return Ok(None);
        }
        let cached_hash = std::fs::read_to_string(hp)?;
        match self.col.replays[id].get("end-hash") {
            Some(h) if h == cached_hash.trim() => Ok(Some(std::fs::read(sp)?)),
            _ => Ok(None),
        }
    }
}

pub fn trunk_start_frame(r: &Replay) -> u64 {
    if r.get("save").is_some() {
        r.get("start-frame").and_then(|s| s.parse().ok()).unwrap_or(0)
    } else {
        0
    }
}

fn parse_hex(s: &str) -> Result<u64> {
    let s = s.trim();
    let h = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u64::from_str_radix(h, 16).map_err(|_| anyhow!("not a hex number: {s}"))
}

/// "PRG:0x1F3A" -> ("PRG", 0x1F3A)
pub fn parse_region_offset(s: &str) -> Result<(String, u64)> {
    let (r, o) = s.split_once(':').ok_or_else(|| anyhow!("expected REGION:0xOFFSET, got {s}"))?;
    Ok((r.trim().to_ascii_uppercase(), parse_hex(o)?))
}
