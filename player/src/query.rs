//! Queries over the local database and the symbol maps, shared by the `dj` query commands
//! and the MCP server (docs/mcp.md). Every result is plain data that serializes to JSON.

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::analysis::{self, AnalyzedFunction, Function};
use crate::cache;
use crate::collection::{Collection, Status};
use crate::coverage::{self, Report};

/// A project opened for queries: the files, the database (read-only) and the symbol maps.
pub struct Project {
    pub col: Collection,
    pub conn: Connection,
    pub setup: String,
    pub rom: Option<Vec<u8>>,
    /// Every function in every symbol map, for names and containing-function lookups.
    pub funcs: Vec<Function>,
    /// PRG bytes (the ROM without its header), when the ROM is known.
    pub prg: Option<Vec<u8>>,
    report: Option<Report>,
}

impl Project {
    pub fn open(dir: &Path, rom: Option<&Path>) -> Result<Project> {
        let col = Collection::open(dir)?;
        let conn = cache::Cache::open_readonly(&col)?;
        let setup: String = conn
            .query_row("SELECT setup FROM contents ORDER BY played_at DESC LIMIT 1", [], |r| r.get(0))
            .context("the local database is empty; run `dj hydrate` first")?;
        let rom_path = match rom {
            Some(r) => Some(r.to_path_buf()),
            None => col.load_local()?.rom,
        };
        let rom = rom_path.and_then(|p| std::fs::read(p).ok());
        let mut funcs = vec![];
        let mut prg = None;
        for s in &col.config.symbols {
            funcs.extend(analysis::load_symbols(&col.root, s, rom.as_deref())?);
            if prg.is_none() {
                if let Some(rom) = &rom {
                    let header = s.rom_header.unwrap_or_else(|| analysis::default_rom_header(&s.format, Some(rom))) as usize;
                    prg = Some(rom[header.min(rom.len())..].to_vec());
                }
            }
        }
        funcs.sort_by_key(|f| (f.region.clone(), f.offset));
        Ok(Project { col, conn, setup, rom, funcs, prg, report: None })
    }

    /// The coverage report, worked out once per opened project.
    pub fn report(&mut self) -> Result<&Report> {
        if self.report.is_none() {
            self.report = Some(coverage::build_report(&self.col, &self.conn, self.rom.as_deref(), Some(&self.setup))?);
        }
        Ok(self.report.as_ref().unwrap())
    }

    fn analyzed(&mut self) -> Result<&[AnalyzedFunction]> {
        let rep = self.report()?;
        if !rep.symbols {
            bail!("this needs a symbol map (config.toml [[symbols]]) and the ROM");
        }
        Ok(&self.report.as_ref().unwrap().detail.analyzed)
    }

    /// Resolves an address: `PRG:0x1F3A` (a region offset), `0x1F3A` (a PRG offset),
    /// `$C123` or `cpu:0xC123` (a CPU address, through the symbol maps), or a symbol
    /// name, optionally `name+0x10`.
    pub fn resolve(&self, what: &str) -> Result<Addr> {
        let what = what.trim();
        let cpu = what.strip_prefix('$').or_else(|| what.strip_prefix("cpu:")).or_else(|| what.strip_prefix("CPU:"));
        if let Some(c) = cpu {
            let c = parse_num_hex(c)?;
            let hits: Vec<&Function> = self.funcs.iter().filter(|f| f.cpu_addr.is_some_and(|b| c >= b && c < b + f.size.max(1))).collect();
            let f = hits.first().ok_or_else(|| anyhow!("CPU address ${c:04X} is not inside any function in the symbol maps; give a ROM offset (PRG:0x1234) instead"))?;
            let offset = f.offset + (c - f.cpu_addr.unwrap());
            let mut a = self.addr(&f.region, offset);
            if hits.len() > 1 {
                a.note = Some(format!("${c:04X} is in {} banks; using {}. Pass a ROM offset to pick another.", hits.len(), a.symbol.clone().unwrap_or_default()));
            }
            return Ok(a);
        }
        if let Ok((region, offset)) = crate::player::parse_region_offset(what) {
            return Ok(self.addr(&region, offset));
        }
        if what.starts_with("0x") || what.starts_with("0X") {
            return Ok(self.addr("PRG", parse_num_hex(what)?));
        }
        let (name, plus) = match what.split_once('+') {
            Some((n, p)) => (n.trim(), parse_num(p.trim())?),
            None => (what, 0),
        };
        let f = self.funcs.iter().find(|f| f.name == name).ok_or_else(|| {
            let close: Vec<&str> = self.funcs.iter().filter(|f| f.name.to_lowercase().contains(&name.to_lowercase())).map(|f| f.name.as_str()).take(5).collect();
            if close.is_empty() {
                anyhow!("{what} is not an address (PRG:0x1234, 0x1234, $C123) or a symbol in the project's symbol maps")
            } else {
                anyhow!("no symbol {name}; similar: {}", close.join(", "))
            }
        })?;
        Ok(self.addr(&f.region.clone(), f.offset + plus))
    }

    /// Describes `region:offset` with its CPU address and containing symbol, when known.
    pub fn addr(&self, region: &str, offset: u64) -> Addr {
        let f = self.function_at(region, offset);
        Addr {
            region: region.to_string(),
            offset,
            text: format!("{region}:{offset:#x}"),
            cpu: f.and_then(|f| f.cpu_addr.map(|c| format!("${:04X}", c + (offset - f.offset)))),
            symbol: f.map(|f| if offset == f.offset { f.name.clone() } else { format!("{}+{:#x}", f.name, offset - f.offset) }),
            note: None,
        }
    }

    pub fn function_at(&self, region: &str, offset: u64) -> Option<&Function> {
        self.funcs.iter().filter(|f| f.region == region && offset >= f.offset && offset < f.offset + f.size.max(1)).min_by_key(|f| f.size)
    }

    fn short(&self, id: &str) -> String {
        id[..8.min(id.len())].to_string()
    }

    fn title(&self, id: &str) -> String {
        self.col.replays.get(id).map(|r| r.title().to_string()).unwrap_or_default()
    }

    fn nearest_snapshot(&self, keys: &std::collections::BTreeMap<String, (Option<String>, Status)>, id: &str, frame: u64) -> Result<Option<u64>> {
        let Some(k) = keys.get(id).and_then(|(k, _)| k.clone()) else { return Ok(None) };
        let n: Option<i64> = self.conn.query_row("SELECT max(frame) FROM frames WHERE content = ?1 AND frame <= ?2", rusqlite::params![k, frame as i64], |r| r.get(0))?;
        Ok(n.map(|n| n as u64))
    }

    /// Segments that run `region` `start..=end`, best first.
    pub fn hits(&self, region: &str, start: u64, end: u64, limit: usize, cursor: usize) -> Result<Page<SegmentHit>> {
        let hits = coverage::first_hits_in(&self.col, &self.conn, &self.setup, region, start, end)?;
        let keys = cache::current_keys(&self.conn, &self.col, &self.setup)?;
        let total = hits.len();
        let mut items = vec![];
        for h in hits.into_iter().skip(cursor).take(limit) {
            items.push(SegmentHit {
                segment: h.id.clone(),
                short: self.short(&h.id),
                title: self.title(&h.id),
                frame: h.frame,
                at: self.addr(region, h.offset).text,
                depth: h.depth,
                status: h.status.label().to_string(),
                counts: h.status != Status::Dirty,
                nearest_snapshot: self.nearest_snapshot(&keys, &h.id, h.frame)?,
            });
        }
        Ok(Page::new(items, total, cursor))
    }

    /// `where_runs`: which segments first run this address, and at which frame.
    pub fn xref(&self, what: &str, limit: usize, cursor: usize) -> Result<Xref> {
        let address = self.resolve(what)?;
        let hits = self.hits(&address.region, address.offset, address.offset, limit, cursor)?;
        Ok(Xref { address, hits })
    }

    /// The headline numbers of the coverage report.
    pub fn summary(&mut self) -> Result<Summary> {
        let rep = self.report()?;
        let t = &rep.totals;
        let pct = |a: usize, b: usize| if b == 0 { 0.0 } else { (a as f64 * 1000.0 / b as f64).round() / 10.0 };
        Ok(Summary {
            setup: rep.setup.clone(),
            symbols: rep.symbols,
            instructions: Ratio { covered: t.instructions_covered, total: t.instructions, percent: pct(t.instructions_covered, t.instructions) },
            blocks: Ratio { covered: t.blocks_covered, total: t.blocks, percent: pct(t.blocks_covered, t.blocks) },
            functions: Ratio { covered: t.functions_entered, total: t.functions, percent: pct(t.functions_entered, t.functions) },
            by_play_alone: Ratio { covered: t.played_only_instructions_covered, total: t.instructions, percent: pct(t.played_only_instructions_covered, t.instructions) },
            unreachable_instructions: t.unreachable_instructions,
            executed_outside_symbols: t.executed_outside_symbols,
            segments: rep.replays.len(),
            segments_left_out: LeftOut { dirty: t.dirty_segments, unplayed: t.unplayed_segments },
            functions_never_entered: rep.functions.iter().filter(|f| f.instructions_covered == 0).count(),
            functions_partly_covered: rep.functions.iter().filter(|f| f.instructions_covered > 0 && f.blocks_covered < f.blocks).count(),
            top_segments: {
                let mut r: Vec<_> = rep.replays.iter().filter(|r| r.unique > 0).collect();
                r.sort_by_key(|r| std::cmp::Reverse(r.unique));
                r.iter().take(5).map(|r| format!("{} {} (+{} only it runs)", &r.id[..8], r.title, r.unique)).collect()
            },
        })
    }

    /// Uncovered code, one row per function that has any, ranked by `sort`.
    pub fn gaps(&mut self, sort: GapSort, only: GapFilter, limit: usize, cursor: usize) -> Result<Page<Gap>> {
        let prg = self.prg.clone().ok_or_else(|| anyhow!("gaps need the ROM (pass --rom once, or set it in .local/local.toml)"))?;
        self.analyzed()?;
        let rep = self.report.as_ref().unwrap();
        let d = &rep.detail;
        let mut rows = vec![];
        for f in &d.analyzed {
            let covered = f.instructions.iter().filter(|i| d.hit(i.offset)).count();
            if covered == f.instructions.len() || f.instructions.is_empty() {
                continue;
            }
            let entered = covered > 0;
            match only {
                GapFilter::All => {}
                GapFilter::Never if entered => continue,
                GapFilter::Partial if !entered => continue,
                _ => {}
            }
            let succ = analysis::block_successors_6502(f, &prg);
            let uncovered: Vec<usize> = (0..f.blocks.len()).filter(|i| !d.hit(f.blocks[*i].offset)).collect();
            let near: HashSet<u64> = f.blocks.iter().zip(&succ).filter(|(b, _)| d.hit(b.offset)).flat_map(|(_, s)| s.iter().copied()).filter(|o| !d.hit(*o)).collect();
            let biggest = uncovered.iter().map(|i| &f.blocks[*i]).filter(|b| near.contains(&b.offset)).max_by_key(|b| b.instructions.len());
            rows.push(Gap {
                function: f.func.name.clone(),
                at: format!("PRG:{:#x}", f.func.offset),
                cpu: f.func.cpu_addr.map(|c| format!("${c:04X}")),
                instructions: f.instructions.len(),
                uncovered_instructions: f.instructions.len() - covered,
                blocks: f.blocks.len(),
                uncovered_blocks: uncovered.len(),
                entered,
                skipped_by_covered_branch: near.len(),
                best_frontier_block: biggest.map(|b| match b.cpu_addr {
                    Some(c) => format!("${c:04X}"),
                    None => format!("PRG:{:#x}", b.offset),
                }),
            });
        }
        match sort {
            GapSort::Size => rows.sort_by(|a, b| b.uncovered_instructions.cmp(&a.uncovered_instructions).then(a.at.cmp(&b.at))),
            GapSort::Nearness => rows.sort_by(|a, b| (b.skipped_by_covered_branch > 0).cmp(&(a.skipped_by_covered_branch > 0)).then(b.entered.cmp(&a.entered)).then(b.uncovered_instructions.cmp(&a.uncovered_instructions))),
        }
        let total = rows.len();
        Ok(Page::new(rows.into_iter().skip(cursor).take(limit).collect(), total, cursor))
    }

    /// One function in detail: its blocks, who enters it, and its covered callers.
    pub fn function(&mut self, what: &str, segments_limit: usize) -> Result<FunctionInfo> {
        let a = self.resolve(what)?;
        let prg = self.prg.clone().ok_or_else(|| anyhow!("this needs the ROM (pass --rom once, or set it in .local/local.toml)"))?;
        self.analyzed()?;
        let rep = self.report.as_ref().unwrap();
        let d = &rep.detail;
        let f = d
            .analyzed
            .iter()
            .find(|f| a.region == f.func.region && a.offset >= f.func.offset && a.offset < f.func.offset + f.func.size.max(1))
            .ok_or_else(|| anyhow!("{} is not inside a function the symbol maps describe", a.text))?;
        let firsts = self.first_frames(&f.func.region, f.func.offset, f.func.offset + f.func.size)?;
        let succ = analysis::block_successors_6502(f, &prg);
        let blocks = f
            .blocks
            .iter()
            .zip(&succ)
            .map(|(b, s)| BlockRow {
                at: format!("PRG:{:#x}", b.offset),
                cpu: b.cpu_addr.map(|c| format!("${c:04X}")),
                instructions: b.instructions.len(),
                covered: d.hit(b.offset),
                first_run: firsts.get(&b.offset).map(|(id, fr)| FirstRun { segment: id.clone(), short: self.short(id), title: self.title(id), frame: *fr }),
                next: s.iter().map(|o| f.blocks.iter().find(|b| b.offset == *o).and_then(|b| b.cpu_addr).map(|c| format!("${c:04X}")).unwrap_or_else(|| format!("PRG:{o:#x}"))).collect(),
            })
            .collect();
        let callers = self.callers(f, &prg);
        let info = FunctionInfo {
            name: f.func.name.clone(),
            at: format!("PRG:{:#x}", f.func.offset),
            cpu: f.func.cpu_addr.map(|c| format!("${c:04X}")),
            size: f.func.size,
            instructions: f.instructions.len(),
            instructions_covered: f.instructions.iter().filter(|i| d.hit(i.offset)).count(),
            blocks,
            callers,
            entered_by: Page { items: vec![], total: 0, next_cursor: None },
        };
        let entry = (info.at.clone(), f.func.region.clone(), f.func.offset);
        let mut info = info;
        info.entered_by = self.hits(&entry.1, entry.2, entry.2, segments_limit, 0)?;
        Ok(info)
    }

    /// The first segment (best first) and frame to run each offset of `start..end`.
    fn first_frames(&self, region: &str, start: u64, end: u64) -> Result<HashMap<u64, (String, u64)>> {
        let keys = cache::current_keys(&self.conn, &self.col, &self.setup)?;
        let mut by_key: HashMap<String, (String, usize, bool)> = HashMap::new();
        for (id, (k, st)) in &keys {
            if let Some(k) = k {
                by_key.insert(k.clone(), (id.clone(), self.col.chain(id).map(|c| c.len()).unwrap_or(0), *st == Status::Dirty));
            }
        }
        let mut st = self.conn.prepare_cached("SELECT content, rom_offset, frame FROM first_hits WHERE region = ?1 AND rom_offset >= ?2 AND rom_offset < ?3")?;
        let rows = st.query_map(rusqlite::params![region, start as i64, end as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)? as u64)))?;
        let mut best: HashMap<u64, ((bool, usize, u64), String, u64)> = HashMap::new();
        for row in rows {
            let (content, off, frame) = row?;
            let Some((id, depth, dirty)) = by_key.get(&content) else { continue };
            let rank = (*dirty, *depth, frame);
            let e = best.entry(off).or_insert((rank, id.clone(), frame));
            if rank < e.0 {
                *e = (rank, id.clone(), frame);
            }
        }
        Ok(best.into_iter().map(|(o, (_, id, f))| (o, (id, f))).collect())
    }

    /// JSR and JMP sites anywhere in the analyzed code that target `f`'s entry.
    fn callers(&self, f: &AnalyzedFunction, prg: &[u8]) -> Vec<Caller> {
        let Some(entry) = f.func.cpu_addr else { return vec![] };
        let d = &self.report.as_ref().unwrap().detail;
        let mut out = vec![];
        for g in &d.analyzed {
            for i in &g.instructions {
                let bytes = &prg[(i.offset as usize).min(prg.len())..((i.offset + i.len as u64) as usize).min(prg.len())];
                if let Some((t, call)) = analysis::call_target_6502(bytes) {
                    if t == entry {
                        out.push(Caller {
                            function: g.func.name.clone(),
                            at: format!("PRG:{:#x}", i.offset),
                            cpu: g.func.cpu_addr.map(|c| format!("${:04X}", c + (i.offset - g.func.offset))),
                            kind: if call { "JSR" } else { "JMP" }.into(),
                            covered: d.hit(i.offset),
                        });
                    }
                }
            }
        }
        out.sort_by_key(|c| !c.covered);
        out
    }

    /// Where to start exploring toward uncovered code. For an address in an uncovered block,
    /// the covered blocks that branch or fall into it; for a function never entered, its
    /// covered callers; with no address, the largest such edges across the ROM.
    pub fn frontier(&mut self, what: Option<&str>, limit: usize, cursor: usize) -> Result<Page<FrontierEdge>> {
        let prg = self.prg.clone().ok_or_else(|| anyhow!("this needs the ROM (pass --rom once, or set it in .local/local.toml)"))?;
        let target = what.map(|w| self.resolve(w)).transpose()?;
        self.analyzed()?;
        let rep = self.report.as_ref().unwrap();
        let d = &rep.detail;
        // (from function, from block, branch instruction offset, target block offset, target size, target function)
        let mut edges: Vec<(usize, usize, u64, u64, usize)> = vec![];
        let mut callers_of: Vec<usize> = vec![];
        for (fi, f) in d.analyzed.iter().enumerate() {
            if let Some(t) = &target {
                let inside = t.region == f.func.region && t.offset >= f.func.offset && t.offset < f.func.offset + f.func.size.max(1);
                if !inside {
                    continue;
                }
                if !f.instructions.iter().any(|i| d.hit(i.offset)) {
                    callers_of.push(fi);
                    continue;
                }
            }
            let succ = analysis::block_successors_6502(f, &prg);
            for (bi, (b, s)) in f.blocks.iter().zip(&succ).enumerate() {
                if !d.hit(b.offset) {
                    continue;
                }
                for o in s {
                    if d.hit(*o) {
                        continue;
                    }
                    if let Some(t) = &target {
                        // Only edges into the block that holds the address, or any block of the function when given its entry.
                        let tb = f.blocks.iter().rposition(|b| b.offset <= t.offset).unwrap_or(0);
                        if t.offset != f.func.offset && f.blocks[tb].offset != *o {
                            continue;
                        }
                    }
                    // Size: the uncovered blocks reachable from the target inside the function.
                    let size = uncovered_reach(f, &succ, *o, d);
                    edges.push((fi, bi, *b.instructions.last().unwrap_or(&b.offset), *o, size));
                }
            }
        }
        let mut out = vec![];
        for (fi, bi, branch, to, size) in &edges {
            let f = &d.analyzed[*fi];
            let b = &f.blocks[*bi];
            let tb = f.blocks.iter().find(|x| x.offset == *to);
            let cpu = |o: u64| f.func.cpu_addr.map(|c| format!("${:04X}", c + (o - f.func.offset)));
            out.push(FrontierEdge {
                kind: "branch".into(),
                function: f.func.name.clone(),
                uncovered: tb.and_then(|x| x.cpu_addr).map(|c| format!("${c:04X}")).unwrap_or_else(|| format!("PRG:{to:#x}")),
                uncovered_instructions: *size,
                from_block: b.cpu_addr.map(|c| format!("${c:04X}")).unwrap_or_else(|| format!("PRG:{:#x}", b.offset)),
                branch: cpu(*branch).unwrap_or_else(|| format!("PRG:{branch:#x}")),
                branch_at: format!("PRG:{branch:#x}"),
                reached_by: vec![],
            });
        }
        for fi in callers_of {
            let f = &d.analyzed[fi];
            for c in self.callers(f, &prg).into_iter().filter(|c| c.covered) {
                out.push(FrontierEdge {
                    kind: c.kind.to_lowercase(),
                    function: f.func.name.clone(),
                    uncovered: f.func.cpu_addr.map(|c| format!("${c:04X}")).unwrap_or_else(|| format!("PRG:{:#x}", f.func.offset)),
                    uncovered_instructions: f.instructions.len(),
                    from_block: c.function.clone(),
                    branch: c.cpu.clone().unwrap_or_else(|| c.at.clone()),
                    branch_at: c.at.clone(),
                    reached_by: vec![],
                });
            }
        }
        out.sort_by(|a, b| b.uncovered_instructions.cmp(&a.uncovered_instructions).then(a.branch_at.cmp(&b.branch_at)));
        let total = out.len();
        let mut page: Vec<FrontierEdge> = out.into_iter().skip(cursor).take(limit).collect();
        for e in &mut page {
            let (region, off) = crate::player::parse_region_offset(&e.branch_at)?;
            e.reached_by = self.hits(&region, off, off, 3, 0)?.items;
        }
        if total == 0 && target.is_some() {
            bail!("no covered code leads directly into it: either it is covered already, or nothing that runs branches or calls there (an indirect jump, an interrupt or a table may reach it)");
        }
        Ok(Page::new(page, total, cursor))
    }
}

fn uncovered_reach(f: &AnalyzedFunction, succ: &[Vec<u64>], start: u64, d: &coverage::Detail) -> usize {
    let mut seen = HashSet::new();
    let mut stack = vec![start];
    let mut n = 0;
    while let Some(o) = stack.pop() {
        if !seen.insert(o) || d.hit(o) {
            continue;
        }
        if let Some(i) = f.blocks.iter().position(|b| b.offset == o) {
            n += f.blocks[i].instructions.len();
            stack.extend(succ[i].iter().copied());
        }
    }
    n
}

fn parse_num_hex(s: &str) -> Result<u64> {
    let s = s.trim();
    let h = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u64::from_str_radix(h, 16).map_err(|_| anyhow!("not a hex number: {s}"))
}

fn parse_num(s: &str) -> Result<u64> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).map_err(|_| anyhow!("not a number: {s}")),
        None => s.parse().map_err(|_| anyhow!("not a number: {s}")),
    }
}

#[derive(Clone, Copy, Debug)]
pub enum GapSort {
    Size,
    Nearness,
}

impl std::str::FromStr for GapSort {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "size" => Ok(GapSort::Size),
            "nearness" | "near" => Ok(GapSort::Nearness),
            _ => bail!("sort is size or nearness"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum GapFilter {
    All,
    Never,
    Partial,
}

impl std::str::FromStr for GapFilter {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "all" => Ok(GapFilter::All),
            "never" | "never-entered" => Ok(GapFilter::Never),
            "partial" => Ok(GapFilter::Partial),
            _ => bail!("only is all, never or partial"),
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Addr {
    pub region: String,
    pub offset: u64,
    /// `REGION:0xOFFSET`
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: usize,
    /// Pass as `cursor` for the next page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<usize>,
}

impl<T> Page<T> {
    fn new(items: Vec<T>, total: usize, cursor: usize) -> Self {
        let next = cursor + items.len();
        Page { items, total, next_cursor: (next < total).then_some(next) }
    }
}

#[derive(Serialize)]
pub struct SegmentHit {
    pub segment: String,
    pub short: String,
    pub title: String,
    /// First frame (inside the segment) that runs it.
    pub frame: u64,
    /// The exact offset that ran first (for a range).
    pub at: String,
    pub depth: usize,
    pub status: String,
    /// Dirty segments don't count toward coverage.
    pub counts: bool,
    /// Nearest snapshot at or before `frame` in the local database.
    pub nearest_snapshot: Option<u64>,
}

#[derive(Serialize)]
pub struct Xref {
    pub address: Addr,
    pub hits: Page<SegmentHit>,
}

#[derive(Serialize)]
pub struct Ratio {
    pub covered: usize,
    pub total: usize,
    pub percent: f64,
}

#[derive(Serialize)]
pub struct LeftOut {
    pub dirty: usize,
    pub unplayed: usize,
}

#[derive(Serialize)]
pub struct Summary {
    pub setup: String,
    pub symbols: bool,
    pub instructions: Ratio,
    pub blocks: Ratio,
    pub functions: Ratio,
    pub by_play_alone: Ratio,
    pub unreachable_instructions: usize,
    pub executed_outside_symbols: usize,
    pub segments: usize,
    pub segments_left_out: LeftOut,
    pub functions_never_entered: usize,
    pub functions_partly_covered: usize,
    pub top_segments: Vec<String>,
}

#[derive(Serialize)]
pub struct Gap {
    pub function: String,
    pub at: String,
    pub cpu: Option<String>,
    pub instructions: usize,
    pub uncovered_instructions: usize,
    pub blocks: usize,
    pub uncovered_blocks: usize,
    /// Whether any of it runs.
    pub entered: bool,
    /// Uncovered blocks that covered code branches or falls into.
    pub skipped_by_covered_branch: usize,
    /// The largest of those, a good argument for `frontier`.
    pub best_frontier_block: Option<String>,
}

#[derive(Serialize)]
pub struct FirstRun {
    pub segment: String,
    pub short: String,
    pub title: String,
    pub frame: u64,
}

#[derive(Serialize)]
pub struct BlockRow {
    pub at: String,
    pub cpu: Option<String>,
    pub instructions: usize,
    pub covered: bool,
    pub first_run: Option<FirstRun>,
    /// Blocks control can go to next, inside the function.
    pub next: Vec<String>,
}

#[derive(Serialize)]
pub struct Caller {
    pub function: String,
    pub at: String,
    pub cpu: Option<String>,
    pub kind: String,
    pub covered: bool,
}

#[derive(Serialize)]
pub struct FunctionInfo {
    pub name: String,
    pub at: String,
    pub cpu: Option<String>,
    pub size: u64,
    pub instructions: usize,
    pub instructions_covered: usize,
    pub blocks: Vec<BlockRow>,
    pub callers: Vec<Caller>,
    pub entered_by: Page<SegmentHit>,
}

#[derive(Serialize)]
pub struct FrontierEdge {
    /// "branch" (a covered block that can branch or fall into uncovered code), or "jsr"/"jmp"
    /// (a covered call to a function that never runs).
    pub kind: String,
    pub function: String,
    pub uncovered: String,
    /// Uncovered instructions reachable from there inside the function.
    pub uncovered_instructions: usize,
    pub from_block: String,
    /// The branch or call instruction that decides it.
    pub branch: String,
    pub branch_at: String,
    /// Segments and frames that run that instruction: where to open a session.
    pub reached_by: Vec<SegmentHit>,
}
