mod analysis;
mod collection;
mod coverage;
mod format;
mod libretro;
mod player;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use collection::{sha256_file, Collection, Config, CoreConfig, HashConfig, LocalConfig, PortConfig, RomConfig, Status, SymbolsConfig};
use format::Replay;
use player::{PlayOptions, ReplayResult, Session};

#[derive(Parser)]
#[command(name = "emucov", version, about = "Play libretro input-replay trees, check state hashes, and measure ROM code coverage")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Args, Clone)]
struct Local {
    /// ROM file (default: the one saved in .emucov/local.toml)
    #[arg(long, global = true)]
    rom: Option<PathBuf>,
    /// libretro core library (default: the one saved in .emucov/local.toml)
    #[arg(long, global = true)]
    core: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a collection pinned to a ROM and a core, with every core option recorded
    Init {
        dir: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        rom: PathBuf,
        #[arg(long)]
        core: PathBuf,
        /// Free-form id of the exact core build (default: the core's own name and version)
        #[arg(long)]
        core_build: Option<String>,
        /// Core option overrides, KEY=VALUE
        #[arg(long = "option")]
        options: Vec<String>,
        /// Symbol map to copy into symbols/ (a cc65 .dbg file or a CSV)
        #[arg(long)]
        symbols: Option<PathBuf>,
    },
    /// Create an empty replay
    New {
        dir: PathBuf,
        #[arg(long)]
        parent: Option<String>,
        #[arg(long, default_value = "")]
        title: String,
        #[arg(long, default_value_t = 60)]
        frames: u64,
    },
    /// Show the replay tree with clean, dirty and unverified marks (no emulation)
    Status { dir: PathBuf },
    /// Play one replay (and its ancestors), check hashes, and save screenshots
    Play {
        dir: PathBuf,
        id: String,
        #[command(flatten)]
        local: Local,
        /// Save the last frame (and any [SCREENSHOT] frames) as PNGs here
        #[arg(long)]
        screenshots: Option<PathBuf>,
        /// Also capture these frames (relative to the replay)
        #[arg(long, value_delimiter = ',')]
        shot: Vec<u64>,
        /// Replay from the trunk instead of a cached ancestor state
        #[arg(long)]
        no_cache: bool,
        /// Write the hashes this run produced into the replay files
        #[arg(long)]
        accept: bool,
    },
    /// Play every replay, check hashes, and record coverage
    Run {
        dir: PathBuf,
        #[command(flatten)]
        local: Local,
        /// Play each chain from its trunk instead of reusing parent states (determinism check)
        #[arg(long)]
        from_scratch: bool,
        /// Write the hashes this run produced into the replay files
        #[arg(long)]
        accept: bool,
        /// Save each replay's last frame to .emucov/screenshots/
        #[arg(long)]
        screenshots: bool,
    },
    /// Report coverage from the latest run
    Report {
        dir: PathBuf,
        #[command(flatten)]
        local: Local,
        /// Write the full report as JSON
        #[arg(long)]
        json: Option<PathBuf>,
        /// Write a CSV of uncovered blocks for collaborators
        #[arg(long)]
        gaps: Option<PathBuf>,
    },
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e:#}");
        std::process::exit(2);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Init { dir, name, rom, core, core_build, options, symbols } => init(&dir, &name, &rom, &core, core_build, &options, symbols.as_deref()),
        Cmd::New { dir, parent, title, frames } => new_replay(&dir, parent, &title, frames),
        Cmd::Status { dir } => status(&dir),
        Cmd::Play { dir, id, local, screenshots, shot, no_cache, accept } => play(&dir, &id, &local, screenshots, shot, no_cache, accept),
        Cmd::Run { dir, local, from_scratch, accept, screenshots } => run(&dir, &local, from_scratch, accept, screenshots),
        Cmd::Report { dir, local, json, gaps } => report(&dir, &local, json, gaps),
    }
}

fn resolve_local(col: &Collection, local: &Local) -> Result<(PathBuf, PathBuf)> {
    let saved = col.load_local()?;
    let rom = local.rom.clone().or(saved.rom).ok_or_else(|| anyhow!("no ROM given; pass --rom (it is then remembered)"))?;
    let core = local.core.clone().or(saved.core).ok_or_else(|| anyhow!("no core given; pass --core (it is then remembered)"))?;
    if local.rom.is_some() || local.core.is_some() {
        col.save_local(&LocalConfig { rom: Some(std::fs::canonicalize(&rom)?), core: Some(std::fs::canonicalize(&core)?) })?;
    }
    Ok((rom, core))
}

fn init(dir: &Path, name: &str, rom: &Path, core_path: &Path, core_build: Option<String>, options: &[String], symbols: Option<&Path>) -> Result<()> {
    if dir.join("collection.toml").exists() {
        bail!("{} already has a collection.toml", dir.display());
    }
    let mut overrides = BTreeMap::new();
    for o in options {
        let (k, v) = o.split_once('=').ok_or_else(|| anyhow!("--option takes KEY=VALUE"))?;
        overrides.insert(k.to_string(), v.to_string());
    }
    std::fs::create_dir_all(dir.join("replays"))?;
    std::fs::create_dir_all(dir.join("blobs"))?;
    let work = dir.join(".emucov");
    let (lib_name, lib_version, effective, regions) = {
        let mut core = libretro::Core::load(core_path, &work, &overrides)?;
        core.load_game(rom)?;
        let mut regions = vec![];
        for r in ["RAM", "SRAM"] {
            if core.memory(libretro::region_memory_id(r).unwrap()).is_some() {
                regions.push(r.to_string());
            }
        }
        if !core.has_connector() {
            eprintln!("note: this core has no coverage connector; replays will play but record no coverage");
        }
        (core.library_name.clone(), core.library_version.clone(), core.effective_options(), regions)
    };
    let mut syms = vec![];
    if let Some(s) = symbols {
        std::fs::create_dir_all(dir.join("symbols"))?;
        let fname = s.file_name().ok_or_else(|| anyhow!("bad symbols path"))?;
        std::fs::copy(s, dir.join("symbols").join(fname))?;
        let format = if s.extension().and_then(|e| e.to_str()) == Some("dbg") { "ca65-dbg" } else { "csv" };
        syms.push(SymbolsConfig { file: format!("symbols/{}", fname.to_string_lossy()), format: format.into(), rom_header: None });
    }
    let config = Config {
        format: 1,
        name: name.to_string(),
        rom: RomConfig { sha256: sha256_file(rom)?, file: rom.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default() },
        core: CoreConfig { name: lib_name.to_lowercase(), build: core_build.unwrap_or_else(|| format!("{lib_name} {lib_version}")), sha256: String::new(), options: effective },
        ports: vec![PortConfig { port: 1, device: "JOYPAD".into() }],
        hash: HashConfig { algorithm: "blake3".into(), regions },
        tick: None,
        symbols: syms,
    };
    std::fs::write(dir.join("collection.toml"), toml::to_string_pretty(&config)?)?;
    let col = Collection::open(dir)?;
    col.save_local(&LocalConfig { rom: Some(std::fs::canonicalize(rom)?), core: Some(std::fs::canonicalize(core_path)?) })?;
    println!("created {}", dir.join("collection.toml").display());
    Ok(())
}

fn new_replay(dir: &Path, parent: Option<String>, title: &str, frames: u64) -> Result<()> {
    let mut col = Collection::open(dir)?;
    let parent = match parent {
        Some(p) => Some(col.resolve(&p)?.to_string()),
        None => None,
    };
    let id = uuid::Uuid::new_v4().to_string();
    let r = Replay::new(&id, parent.as_deref(), title, frames);
    col.replays.insert(id.clone(), r);
    col.save_replay(&id)?;
    println!("{}", col.paths[&id].display());
    Ok(())
}

fn status(dir: &Path) -> Result<()> {
    let col = Collection::open(dir)?;
    let mut counts = BTreeMap::new();
    for (id, depth) in col.tree_order() {
        let r = &col.replays[id];
        let st = col.status(id);
        *counts.entry(st.label()).or_insert(0) += 1;
        let mark = match st {
            Status::Clean => " ",
            Status::Dirty => "!",
            Status::Unverified => "?",
        };
        println!("{mark} {}{} {:>6}f  {}{}", "  ".repeat(depth), &id[..8], r.frames()?, r.title(), if r.is_poked() { "  [poked]" } else { "" });
    }
    println!("{} replays: {:?}", col.replays.len(), counts);
    Ok(())
}

/// Compares a result with the replay's recorded hashes. Returns the outcome label.
fn judge(col: &Collection, res: &ReplayResult) -> &'static str {
    let r = &col.replays[&res.id];
    if let Some(s) = r.get("start-hash") {
        if s != res.start_hash {
            return "dirty";
        }
    }
    let cps = r.checkpoints().unwrap_or_default();
    for (f, h) in &cps {
        if let Some((_, got)) = res.checkpoints.iter().find(|(gf, _)| gf == f) {
            if got != h {
                return "changed";
            }
        }
    }
    match r.get("end-hash") {
        None => "new",
        Some(e) if e == res.end_hash => "ok",
        Some(_) => "changed",
    }
}

fn accept(col: &mut Collection, res: &ReplayResult) -> Result<()> {
    let r = col.replays.get_mut(&res.id).unwrap();
    r.set("start-frame", &res.start_frame.to_string());
    r.set("start-hash", &res.start_hash);
    r.set("end-hash", &res.end_hash);
    if !res.checkpoints.is_empty() {
        r.remove_all("checkpoint");
        for (f, h) in &res.checkpoints {
            r.add("checkpoint", &format!("{f} {h}"));
        }
    }
    col.save_replay(&res.id)
}

fn save_shots(res: &ReplayResult, dir: &Path) -> Result<()> {
    for (f, frame) in &res.screenshots {
        let p = dir.join(format!("{}-{:06}.png", &res.id[..8], f));
        libretro::write_png(frame, &p)?;
        println!("  screenshot {}", p.display());
    }
    Ok(())
}

fn play(dir: &Path, id: &str, local: &Local, shots: Option<PathBuf>, extra: Vec<u64>, no_cache: bool, do_accept: bool) -> Result<()> {
    let mut col = Collection::open(dir)?;
    let (rom, core) = resolve_local(&col, local)?;
    let id = col.resolve(id)?.to_string();
    let results = {
        let mut s = Session::open(&col, &rom, &core)?;
        let opts = PlayOptions { end_screenshot: shots.is_some(), extra_shots: extra };
        s.play_chain(&id, &opts, !no_cache)?
    };
    let mut bad = false;
    for res in &results {
        let outcome = judge(&col, res);
        println!("{:<8} {} {}  end {}", outcome, &res.id[..8], col.replays[&res.id].title(), &res.end_hash[..23]);
        if let Some(d) = &shots {
            save_shots(res, d)?;
        }
        if matches!(outcome, "dirty" | "changed") {
            bad = true;
        }
    }
    if do_accept {
        for res in &results {
            accept(&mut col, res)?;
        }
        println!("hashes written");
    } else if bad {
        bail!("hashes differ from the recorded ones (use --accept to record the new ones)");
    }
    Ok(())
}

fn run(dir: &Path, local: &Local, from_scratch: bool, do_accept: bool, screenshots: bool) -> Result<()> {
    let col = Collection::open(dir)?;
    let (rom, core) = resolve_local(&col, local)?;
    let store = coverage::Store::open(&col.local_dir().join("coverage.sqlite"))?;
    let shot_dir = col.local_dir().join("screenshots");
    // What --accept needs, kept after the session (which borrows the collection) ends.
    let mut accepted: Vec<(ReplayResult, &'static str)> = vec![];
    let started = std::time::Instant::now();
    let mut total_frames = 0u64;
    {
        let mut s = Session::open(&col, &rom, &core)?;
        if !s.core.has_connector() {
            eprintln!("note: this core has no coverage connector; recording hashes only");
        }
        let run_id = store.begin_run(&s.setup_id, &col.config.core.build, from_scratch)?;
        let opts = PlayOptions { end_screenshot: screenshots, extra_shots: vec![] };
        s.play_all(&opts, from_scratch, |s, res| {
            let outcome = judge(s.col, res);
            let depth = s.col.chain(&res.id)?.len() - 1;
            println!(
                "{:<8} {}{} {}  {}f{}",
                outcome,
                "  ".repeat(depth),
                &res.id[..8],
                s.col.replays[&res.id].title(),
                res.frames,
                res.ticks.map(|t| format!(", {t} ticks")).unwrap_or_default()
            );
            if screenshots {
                save_shots(res, &shot_dir)?;
            }
            store.record(run_id, s.col, res, outcome)?;
            total_frames += res.frames;
            accepted.push((
                ReplayResult {
                    id: res.id.clone(),
                    start_frame: res.start_frame,
                    frames: res.frames,
                    start_hash: res.start_hash.clone(),
                    end_hash: res.end_hash.clone(),
                    checkpoints: res.checkpoints.clone(),
                    ticks: res.ticks,
                    poked: res.poked,
                    coverage: vec![],
                    screenshots: vec![],
                    end_state: vec![],
                },
                outcome,
            ));
            Ok(())
        })?;
    }
    let secs = started.elapsed().as_secs_f64();
    println!("played {total_frames} frames in {secs:.2}s ({:.0} frames/s)", total_frames as f64 / secs.max(1e-9));

    let mut col = col;
    let bad = accepted.iter().filter(|(_, o)| matches!(*o, "dirty" | "changed")).count();
    if do_accept {
        for (res, _) in &accepted {
            accept(&mut col, res)?;
        }
        println!("hashes written for {} replays", accepted.len());
    } else if bad > 0 {
        bail!("{bad} replays differ from their recorded hashes (use --accept to record the new ones)");
    }
    Ok(())
}

fn report(dir: &Path, local: &Local, json: Option<PathBuf>, gaps: Option<PathBuf>) -> Result<()> {
    let col = Collection::open(dir)?;
    let rom = match resolve_local(&col, local) {
        Ok((rom, _)) => Some(std::fs::read(&rom).with_context(|| format!("reading {}", rom.display()))?),
        Err(_) => None,
    };
    let store = coverage::Store::open(&col.local_dir().join("coverage.sqlite"))?;
    let rep = coverage::build_report(&col, &store, rom.as_deref())?;
    coverage::print_report(&rep, &col);
    if let Some(j) = json {
        std::fs::write(&j, serde_json::to_string_pretty(&rep)?)?;
        println!("wrote {}", j.display());
    }
    if let Some(g) = gaps {
        coverage::write_gaps_csv(&rep, &g)?;
        println!("wrote {}", g.display());
    }
    Ok(())
}
