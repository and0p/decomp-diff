//! Local coverage database and reports (SPEC.md section 9).

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::analysis::{self, AnalyzedFunction};
use crate::collection::Collection;
use crate::player::ReplayResult;

pub const FLAG_OPCODE: u8 = 0x01;

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS runs(
                id INTEGER PRIMARY KEY, created TEXT NOT NULL, setup TEXT NOT NULL,
                core TEXT NOT NULL, from_scratch INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS replay_runs(
                run_id INTEGER NOT NULL, replay_id TEXT NOT NULL, parent TEXT, title TEXT,
                start_frame INTEGER, frames INTEGER, start_hash TEXT, end_hash TEXT,
                ticks INTEGER, poked INTEGER, outcome TEXT,
                PRIMARY KEY(run_id, replay_id));
             CREATE TABLE IF NOT EXISTS coverage(
                run_id INTEGER NOT NULL, replay_id TEXT NOT NULL, region TEXT NOT NULL, flags BLOB NOT NULL,
                PRIMARY KEY(run_id, replay_id, region));",
        )?;
        Ok(Store { conn })
    }

    pub fn begin_run(&self, setup: &str, core: &str, from_scratch: bool) -> Result<i64> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
        self.conn.execute(
            "INSERT INTO runs(created, setup, core, from_scratch) VALUES (?1, ?2, ?3, ?4)",
            params![now.to_string(), setup, core, from_scratch as i32],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn record(&self, run: i64, col: &Collection, res: &ReplayResult, outcome: &str) -> Result<()> {
        let r = &col.replays[&res.id];
        self.conn.execute(
            "INSERT OR REPLACE INTO replay_runs VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                run,
                res.id,
                r.parent(),
                r.title(),
                res.start_frame as i64,
                res.frames as i64,
                res.start_hash,
                res.end_hash,
                res.ticks.map(|t| t as i64),
                res.poked as i32,
                outcome
            ],
        )?;
        for reg in &res.coverage {
            self.conn.execute("INSERT OR REPLACE INTO coverage VALUES (?1,?2,?3,?4)", params![run, res.id, reg.name, reg.flags])?;
        }
        Ok(())
    }

    pub fn latest_run(&self) -> Result<Option<i64>> {
        Ok(self.conn.query_row("SELECT MAX(id) FROM runs", [], |r| r.get(0))?)
    }

    /// replay id -> region -> flags
    pub fn load_run(&self, run: i64) -> Result<BTreeMap<String, BTreeMap<String, Vec<u8>>>> {
        let mut out: BTreeMap<String, BTreeMap<String, Vec<u8>>> = BTreeMap::new();
        let mut st = self.conn.prepare("SELECT replay_id, region, flags FROM coverage WHERE run_id = ?1")?;
        let rows = st.query_map([run], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?)))?;
        for row in rows {
            let (id, region, flags) = row?;
            out.entry(id).or_default().insert(region, flags);
        }
        Ok(out)
    }

    pub fn load_ticks(&self, run: i64) -> Result<HashMap<String, Option<i64>>> {
        let mut st = self.conn.prepare("SELECT replay_id, ticks FROM replay_runs WHERE run_id = ?1")?;
        let rows = st.query_map([run], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)))?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }
}

// ---- report -------------------------------------------------------------------

#[derive(Serialize)]
pub struct Totals {
    pub functions: usize,
    pub functions_entered: usize,
    pub blocks: usize,
    pub blocks_covered: usize,
    pub instructions: usize,
    pub instructions_covered: usize,
    pub played_only_instructions_covered: usize,
    pub executed_bytes_by_region: BTreeMap<String, usize>,
    pub executed_outside_symbols: usize,
}

#[derive(Serialize)]
pub struct ReplayRow {
    pub id: String,
    pub title: String,
    pub depth: usize,
    pub poked: bool,
    pub ticks: Option<i64>,
    /// Instructions (or opcode bytes, without symbols) run during this replay's own frames.
    pub own: usize,
    /// ... that no ancestor ran.
    pub new_vs_ancestors: usize,
    /// ... that no other replay ran.
    pub unique: usize,
}

#[derive(Serialize)]
pub struct FunctionRow {
    pub name: String,
    pub offset: String,
    pub cpu_addr: Option<String>,
    pub instructions: usize,
    pub instructions_covered: usize,
    pub blocks: usize,
    pub blocks_covered: usize,
    pub uncovered_blocks: Vec<String>,
}

#[derive(Serialize)]
pub struct Report {
    pub run: i64,
    pub symbols: bool,
    pub totals: Totals,
    pub replays: Vec<ReplayRow>,
    pub smallest_set: Vec<String>,
    pub functions: Vec<FunctionRow>,
}

fn union(a: &mut Vec<u8>, b: &[u8]) {
    if a.len() < b.len() {
        a.resize(b.len(), 0);
    }
    for (x, y) in a.iter_mut().zip(b) {
        *x |= *y;
    }
}

fn opcode_set(flags: &[u8]) -> Vec<bool> {
    flags.iter().map(|f| f & FLAG_OPCODE != 0).collect()
}

pub fn build_report(col: &Collection, store: &Store, rom: Option<&[u8]>) -> Result<Report> {
    let run = store.latest_run()?.context("no runs recorded yet; run `emucov run` first")?;
    let cov = store.load_run(run)?;
    let ticks = store.load_ticks(run)?;
    if cov.is_empty() {
        bail!("the latest run recorded no coverage (was the core built with a coverage connector?)");
    }

    // Symbols -> analyzed functions (PRG only for now, 6502).
    let mut analyzed: Vec<AnalyzedFunction> = vec![];
    for s in &col.config.symbols {
        let path = col.root.join(&s.file);
        let funcs = match s.format.as_str() {
            "ca65-dbg" => analysis::load_ca65_dbg(&path, s.rom_header.unwrap_or(16))?,
            "csv" => analysis::load_csv(&path)?,
            other => bail!("unknown symbols format {other}"),
        };
        let Some(rom) = rom else { bail!("symbols need the ROM to disassemble") };
        let header = s.rom_header.unwrap_or(16) as usize;
        let prg = &rom[header.min(rom.len())..];
        analyzed.extend(analysis::analyze_6502(&funcs, prg));
    }
    let symbols = !analyzed.is_empty();

    // Units of coverage: instruction offsets in PRG when we have symbols, else opcode bytes.
    let units: Vec<u64> = if symbols {
        analyzed.iter().flat_map(|f| f.instructions.iter().map(|i| i.offset)).collect()
    } else {
        let max = cov.values().filter_map(|m| m.get("PRG")).map(|f| f.len()).max().unwrap_or(0);
        (0..max as u64).collect()
    };
    let hit = |flags: Option<&Vec<u8>>, off: u64| flags.and_then(|f| f.get(off as usize)).map(|b| b & FLAG_OPCODE != 0).unwrap_or(false);

    // Unions.
    let mut all_prg: Vec<u8> = vec![];
    let mut played_prg: Vec<u8> = vec![];
    let mut by_region: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (id, regions) in &cov {
        for (name, flags) in regions {
            union(by_region.entry(name.clone()).or_default(), flags);
        }
        if let Some(p) = regions.get("PRG") {
            union(&mut all_prg, p);
            if !col.replays.get(id).map(|r| r.is_poked()).unwrap_or(false) {
                union(&mut played_prg, p);
            }
        }
    }
    let executed_bytes_by_region = by_region.iter().map(|(k, v)| (k.clone(), opcode_set(v).iter().filter(|b| **b).count())).collect();
    let in_symbols: std::collections::HashSet<u64> = units.iter().copied().collect();
    let executed_outside_symbols = if symbols {
        opcode_set(&all_prg).iter().enumerate().filter(|(i, b)| **b && !in_symbols.contains(&(*i as u64))).count()
    } else {
        0
    };

    // Per-replay rows.
    let unit_hits = |flags: Option<&Vec<u8>>| -> Vec<bool> { units.iter().map(|u| hit(flags, *u)).collect() };
    let per_replay: HashMap<&str, Vec<bool>> = cov.iter().map(|(id, m)| (id.as_str(), unit_hits(m.get("PRG")))).collect();
    let mut replays = vec![];
    let empty = vec![false; units.len()];
    for (id, depth) in col.tree_order() {
        let mine = per_replay.get(id).unwrap_or(&empty);
        let mut anc = vec![false; units.len()];
        for a in col.chain(id)?.iter().rev().skip(1) {
            if let Some(v) = per_replay.get(a) {
                for (x, y) in anc.iter_mut().zip(v) {
                    *x |= *y;
                }
            }
        }
        let mut others = vec![false; units.len()];
        for (oid, v) in &per_replay {
            if *oid != id {
                for (x, y) in others.iter_mut().zip(v) {
                    *x |= *y;
                }
            }
        }
        let r = &col.replays[id];
        replays.push(ReplayRow {
            id: id.to_string(),
            title: r.title().to_string(),
            depth,
            poked: r.is_poked(),
            ticks: ticks.get(id).copied().flatten(),
            own: mine.iter().filter(|b| **b).count(),
            new_vs_ancestors: mine.iter().zip(&anc).filter(|(m, a)| **m && !**a).count(),
            unique: mine.iter().zip(&others).filter(|(m, o)| **m && !**o).count(),
        });
    }

    // Smallest set of replays (greedy): each pick brings its whole chain.
    let chain_cov: HashMap<&str, Vec<bool>> = per_replay
        .keys()
        .map(|id| {
            let mut v = vec![false; units.len()];
            for c in col.chain(id).unwrap_or_default() {
                if let Some(p) = per_replay.get(c) {
                    for (x, y) in v.iter_mut().zip(p) {
                        *x |= *y;
                    }
                }
            }
            (*id, v)
        })
        .collect();
    let mut got = vec![false; units.len()];
    let mut smallest_set = vec![];
    loop {
        let best = chain_cov
            .iter()
            .map(|(id, v)| (v.iter().zip(&got).filter(|(a, g)| **a && !**g).count(), *id))
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(a.1)));
        match best {
            Some((gain, id)) if gain > 0 => {
                for (g, v) in got.iter_mut().zip(&chain_cov[id]) {
                    *g |= *v;
                }
                smallest_set.push(id.to_string());
            }
            _ => break,
        }
    }

    // Function rows.
    let all_flags = Some(&all_prg);
    let mut functions = vec![];
    let (mut nf, mut nfe, mut nb, mut nbc, mut ni, mut nic, mut npi) = (0, 0, 0, 0, 0, 0, 0);
    for f in &analyzed {
        let icov = f.instructions.iter().filter(|i| hit(all_flags, i.offset)).count();
        let pcov = f.instructions.iter().filter(|i| hit(Some(&played_prg), i.offset)).count();
        let mut uncovered = vec![];
        let mut bcov = 0;
        for b in &f.blocks {
            if hit(all_flags, b.offset) {
                bcov += 1;
            } else {
                uncovered.push(match b.cpu_addr {
                    Some(c) => format!("${c:04X}"),
                    None => format!("PRG:{:#x}", b.offset),
                });
            }
        }
        nf += 1;
        if icov > 0 {
            nfe += 1;
        }
        nb += f.blocks.len();
        nbc += bcov;
        ni += f.instructions.len();
        nic += icov;
        npi += pcov;
        functions.push(FunctionRow {
            name: f.func.name.clone(),
            offset: format!("{:#x}", f.func.offset),
            cpu_addr: f.func.cpu_addr.map(|c| format!("${c:04X}")),
            instructions: f.instructions.len(),
            instructions_covered: icov,
            blocks: f.blocks.len(),
            blocks_covered: bcov,
            uncovered_blocks: uncovered,
        });
    }
    if !symbols {
        ni = units.len();
        nic = opcode_set(&all_prg).iter().filter(|b| **b).count();
        npi = opcode_set(&played_prg).iter().filter(|b| **b).count();
    }

    Ok(Report {
        run,
        symbols,
        totals: Totals {
            functions: nf,
            functions_entered: nfe,
            blocks: nb,
            blocks_covered: nbc,
            instructions: ni,
            instructions_covered: nic,
            played_only_instructions_covered: npi,
            executed_bytes_by_region,
            executed_outside_symbols,
        },
        replays,
        smallest_set,
        functions,
    })
}

fn pct(a: usize, b: usize) -> String {
    if b == 0 {
        "-".into()
    } else {
        format!("{:.1}%", a as f64 * 100.0 / b as f64)
    }
}

pub fn print_report(rep: &Report, col: &Collection) {
    let t = &rep.totals;
    println!("Coverage, run {}", rep.run);
    if rep.symbols {
        println!("  functions entered    {:>6} / {:<6} {}", t.functions_entered, t.functions, pct(t.functions_entered, t.functions));
        println!("  basic blocks         {:>6} / {:<6} {}", t.blocks_covered, t.blocks, pct(t.blocks_covered, t.blocks));
        println!("  instructions         {:>6} / {:<6} {}", t.instructions_covered, t.instructions, pct(t.instructions_covered, t.instructions));
        println!("  ... by play alone    {:>6} / {:<6} {}", t.played_only_instructions_covered, t.instructions, pct(t.played_only_instructions_covered, t.instructions));
        if t.executed_outside_symbols > 0 {
            println!("  executed PRG bytes outside known functions: {}", t.executed_outside_symbols);
        }
    } else {
        println!("  (no symbol map: counting executed opcode bytes)");
    }
    for (region, n) in &t.executed_bytes_by_region {
        println!("  opcode bytes run in {region:<5} {n}");
    }
    println!();
    println!("  {:<44} {:>7} {:>7} {:>7} {:>7}", "replay", "own", "new", "unique", "ticks");
    for r in &rep.replays {
        let name = format!("{}{} {}", "  ".repeat(r.depth), &r.id[..8], r.title);
        let name: String = name.chars().take(44).collect();
        let ticks = r.ticks.map(|t| t.to_string()).unwrap_or_else(|| "-".into());
        println!("  {:<44} {:>7} {:>7} {:>7} {:>7}{}", name, r.own, r.new_vs_ancestors, r.unique, ticks, if r.poked { "  poked" } else { "" });
    }
    println!();
    let titles: Vec<String> = rep.smallest_set.iter().map(|id| format!("{} {}", &id[..8], col.replays[id].title())).collect();
    println!("  smallest set keeping all coverage ({} of {}): {}", rep.smallest_set.len(), rep.replays.len(), titles.join("; "));
    if rep.symbols {
        let never: Vec<&FunctionRow> = rep.functions.iter().filter(|f| f.instructions_covered == 0).collect();
        let partial: Vec<&FunctionRow> = rep.functions.iter().filter(|f| f.instructions_covered > 0 && f.blocks_covered < f.blocks).collect();
        println!("  functions never entered: {}; partly covered: {}", never.len(), partial.len());
    }
}

/// Gap list for collaborators: one row per uncovered block.
pub fn write_gaps_csv(rep: &Report, path: &Path) -> Result<()> {
    let mut s = String::from("function,cpu_addr,prg_offset,instructions,instructions_covered,blocks,blocks_covered,uncovered_block\n");
    for f in &rep.functions {
        if f.blocks_covered == f.blocks {
            continue;
        }
        for b in &f.uncovered_blocks {
            s.push_str(&format!(
                "{},{},{},{},{},{},{},{}\n",
                f.name,
                f.cpu_addr.clone().unwrap_or_default(),
                f.offset,
                f.instructions,
                f.instructions_covered,
                f.blocks,
                f.blocks_covered,
                b
            ));
        }
    }
    std::fs::write(path, s)?;
    Ok(())
}
