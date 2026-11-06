//! Symbol maps and static analysis used to roll byte coverage up into
//! instructions, basic blocks and functions (.agents/SPEC.md section 9).

use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Function {
    pub name: String,
    pub region: String,
    /// Offset into the region (e.g. PRG ROM offset).
    pub offset: u64,
    pub size: u64,
    /// Address the CPU sees for `offset`, when known (needed to resolve branch targets).
    pub cpu_addr: Option<u64>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Instruction {
    pub offset: u64,
    pub len: u8,
}

#[derive(Clone, Debug)]
pub struct Block {
    /// Region offset of the first instruction.
    pub offset: u64,
    /// Region offsets of every instruction in the block.
    pub instructions: Vec<u64>,
    pub cpu_addr: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct AnalyzedFunction {
    pub func: Function,
    pub instructions: Vec<Instruction>,
    pub blocks: Vec<Block>,
}

// ---- symbol maps -------------------------------------------------------------

/// Reads a cc65 linker debug file (`ld65 --dbgfile`). Each `.proc` scope becomes a
/// function. Code labels in code segments that are not inside a `.proc` become
/// functions that run to the next label or proc.
pub fn load_ca65_dbg(path: &Path, rom_header: u64) -> Result<Vec<Function>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    struct Seg {
        name: String,
        start: u64,
        size: u64,
        ooffs: Option<u64>,
    }
    struct Sym {
        name: String,
        val: u64,
        seg: Option<u64>,
        scope: u64,
        is_label: bool,
    }
    struct Scope {
        name: String,
        parent: Option<u64>,
        size: u64,
        sym: Option<u64>,
    }
    let mut segs: HashMap<u64, Seg> = HashMap::new();
    let mut syms: HashMap<u64, Sym> = HashMap::new();
    let mut scopes: HashMap<u64, Scope> = HashMap::new();
    for line in text.lines() {
        let Some((kind, rest)) = line.split_once('\t') else { continue };
        let kv = parse_kv(rest);
        let num = |k: &str| kv.get(k).and_then(|v| parse_dbg_num(v));
        let id = match num("id") {
            Some(i) => i,
            None => continue,
        };
        match kind {
            "seg" => {
                segs.insert(id, Seg { name: kv.get("name").cloned().unwrap_or_default(), start: num("start").unwrap_or(0), size: num("size").unwrap_or(0), ooffs: num("ooffs") });
            }
            "sym" => {
                syms.insert(
                    id,
                    Sym {
                        name: kv.get("name").cloned().unwrap_or_default(),
                        val: num("val").unwrap_or(0),
                        seg: num("seg"),
                        scope: num("scope").unwrap_or(0),
                        is_label: kv.get("type").map(|t| t == "lab").unwrap_or(false),
                    },
                );
            }
            "scope" => {
                if kv.get("type").map(|t| t == "scope").unwrap_or(false) {
                    scopes.insert(id, Scope { name: kv.get("name").cloned().unwrap_or_default(), parent: num("parent"), size: num("size").unwrap_or(0), sym: num("sym") });
                }
            }
            _ => {}
        }
    }
    let is_code_seg = |s: &Seg| s.name.to_ascii_uppercase().contains("CODE") && s.ooffs.is_some();
    let to_offset = |seg: &Seg, val: u64| -> Option<u64> {
        let o = seg.ooffs?;
        if val < seg.start || val >= seg.start + seg.size {
            return None;
        }
        (o + (val - seg.start)).checked_sub(rom_header)
    };
    let full_name = |mut id: u64| -> String {
        let mut parts = vec![];
        while let Some(s) = scopes.get(&id) {
            if !s.name.is_empty() {
                parts.push(s.name.clone());
            }
            match s.parent {
                Some(p) => id = p,
                None => break,
            }
        }
        parts.reverse();
        parts.join("::")
    };

    let mut funcs = vec![];
    let mut covered: Vec<(u64, u64)> = vec![];
    for (sid, sc) in &scopes {
        let Some(sym) = sc.sym.and_then(|s| syms.get(&s)) else { continue };
        let Some(seg) = sym.seg.and_then(|s| segs.get(&s)) else { continue };
        if !is_code_seg(seg) || sc.size == 0 {
            continue;
        }
        let Some(off) = to_offset(seg, sym.val) else { continue };
        funcs.push(Function { name: full_name(*sid), region: "PRG".into(), offset: off, size: sc.size, cpu_addr: Some(sym.val) });
        covered.push((off, off + sc.size));
    }
    // Top-level labels in code segments outside any proc.
    let mut labels: Vec<(u64, u64, String, u64)> = vec![]; // (offset, cpu, name, seg id)
    for s in syms.values() {
        if !s.is_label {
            continue;
        }
        let Some(seg_id) = s.seg else { continue };
        let seg = &segs[&seg_id];
        if !is_code_seg(seg) {
            continue;
        }
        let Some(off) = to_offset(seg, s.val) else { continue };
        if covered.iter().any(|(a, b)| off >= *a && off < *b) {
            continue;
        }
        // Only labels whose scope is a module's top level (no named parent scope).
        let parent_named = scopes.get(&s.scope).map(|sc| !sc.name.is_empty()).unwrap_or(false);
        if parent_named {
            continue;
        }
        labels.push((off, s.val, s.name.clone(), seg_id));
    }
    labels.sort();
    labels.dedup_by_key(|l| l.0);
    let mut starts: BTreeSet<u64> = funcs.iter().map(|f| f.offset).collect();
    starts.extend(labels.iter().map(|l| l.0));
    for (off, cpu, name, seg_id) in &labels {
        let seg = &segs[seg_id];
        let seg_end = to_offset(seg, seg.start).unwrap() + seg.size;
        let next = starts.range(off + 1..).next().copied().unwrap_or(seg_end).min(seg_end);
        if next > *off {
            funcs.push(Function { name: name.clone(), region: "PRG".into(), offset: *off, size: next - off, cpu_addr: Some(*cpu) });
        }
    }
    funcs.sort_by_key(|f| f.offset);
    if funcs.is_empty() {
        bail!("{} has no code symbols", path.display());
    }
    Ok(funcs)
}

fn parse_dbg_num(v: &str) -> Option<u64> {
    if let Some(h) = v.strip_prefix("0x") {
        u64::from_str_radix(h, 16).ok()
    } else {
        v.split('+').next()?.parse().ok()
    }
}

fn parse_kv(s: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut key = String::new();
    let mut val = String::new();
    let mut in_key = true;
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            '=' if in_key && !quoted => in_key = false,
            ',' if !quoted => {
                out.insert(std::mem::take(&mut key), std::mem::take(&mut val));
                in_key = true;
            }
            _ if in_key => key.push(c),
            _ => val.push(c),
        }
    }
    if !key.is_empty() {
        out.insert(key, val);
    }
    out
}

/// CSV symbol map: `name,region,offset,size[,cpu_addr]`, numbers in hex.
pub fn load_csv(path: &Path) -> Result<Vec<Function>> {
    let text = std::fs::read_to_string(path)?;
    let mut out = vec![];
    for (n, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || (n == 0 && t.starts_with("name,")) {
            continue;
        }
        let f: Vec<&str> = t.split(',').map(|s| s.trim()).collect();
        if f.len() < 4 {
            bail!("{}:{}: expected name,region,offset,size", path.display(), n + 1);
        }
        let hex = |s: &str| u64::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|_| anyhow!("{}:{}: bad hex {s}", path.display(), n + 1));
        out.push(Function {
            name: f[0].to_string(),
            region: f[1].to_ascii_uppercase(),
            offset: hex(f[2])?,
            size: hex(f[3])?,
            cpu_addr: f.get(4).map(|s| hex(s)).transpose()?,
        });
    }
    out.sort_by_key(|f| f.offset);
    Ok(out)
}

// ---- 6502 ---------------------------------------------------------------------

/// Instruction length for each 6502 opcode, including the stable undocumented ones.
pub fn op_len_6502(op: u8) -> u8 {
    let row = op >> 4;
    let odd_row = row & 1 == 1;
    match op & 0x0f {
        0x0 => {
            if odd_row {
                2
            } else {
                match op {
                    0x20 => 3,
                    0x00 | 0x40 | 0x60 => 1,
                    _ => 2,
                }
            }
        }
        0x1 | 0x3 | 0x4 | 0x5 | 0x6 | 0x7 => 2,
        0x2 => match op {
            0x82 | 0xA2 | 0xC2 | 0xE2 => 2,
            _ => 1, // KIL / JAM
        },
        0x8 | 0xA => 1,
        0x9 | 0xB => {
            if odd_row {
                3
            } else {
                2
            }
        }
        _ => 3, // 0xC..0xF
    }
}

enum Flow {
    Next,
    Branch(u64),
    Jump(Option<u64>),
    Stop,
}

fn flow_6502(op: u8, bytes: &[u8], cpu: u64) -> Flow {
    match op {
        0x10 | 0x30 | 0x50 | 0x70 | 0x90 | 0xB0 | 0xD0 | 0xF0 => {
            let rel = bytes.get(1).copied().unwrap_or(0) as i8 as i64;
            Flow::Branch(((cpu as i64 + 2 + rel) & 0xffff) as u64)
        }
        0x4C => Flow::Jump(Some(u16::from_le_bytes([bytes.get(1).copied().unwrap_or(0), bytes.get(2).copied().unwrap_or(0)]) as u64)),
        0x6C => Flow::Jump(None),
        0x00 | 0x40 | 0x60 => Flow::Stop,
        op if op & 0x0f == 0x2 && !matches!(op, 0x82 | 0xA2 | 0xC2 | 0xE2) => Flow::Stop,
        _ => Flow::Next,
    }
}

/// Linear-sweep disassembly of each function into instructions and basic blocks.
/// Data embedded in a function's range can throw the sweep off; ca65 code keeps
/// data in RODATA, so this is rarely a problem there.
pub fn analyze_6502(funcs: &[Function], rom: &[u8]) -> Vec<AnalyzedFunction> {
    let mut out = vec![];
    for f in funcs {
        let mut insns = vec![];
        let mut leaders: BTreeSet<u64> = BTreeSet::new();
        leaders.insert(f.offset);
        let end = (f.offset + f.size).min(rom.len() as u64);
        let mut off = f.offset;
        let cpu_of = |o: u64| f.cpu_addr.map(|c| c + (o - f.offset));
        let off_of_cpu = |cpu: u64| -> Option<u64> {
            let base = f.cpu_addr?;
            if cpu >= base && cpu < base + f.size {
                Some(f.offset + (cpu - base))
            } else {
                None
            }
        };
        while off < end {
            let op = rom[off as usize];
            let len = op_len_6502(op);
            if off + len as u64 > end {
                break;
            }
            insns.push(Instruction { offset: off, len });
            let bytes = &rom[off as usize..(off + len as u64) as usize];
            let next = off + len as u64;
            match flow_6502(op, bytes, cpu_of(off).unwrap_or(0)) {
                Flow::Next => {}
                Flow::Branch(t) => {
                    if f.cpu_addr.is_some() {
                        if let Some(o) = off_of_cpu(t) {
                            leaders.insert(o);
                        }
                    }
                    leaders.insert(next);
                }
                Flow::Jump(t) => {
                    if let Some(o) = t.and_then(off_of_cpu) {
                        leaders.insert(o);
                    }
                    leaders.insert(next);
                }
                Flow::Stop => {
                    leaders.insert(next);
                }
            }
            off = next;
        }
        let mut blocks: Vec<Block> = vec![];
        for i in &insns {
            if leaders.contains(&i.offset) || blocks.is_empty() {
                blocks.push(Block { offset: i.offset, instructions: vec![], cpu_addr: cpu_of(i.offset) });
            }
            blocks.last_mut().unwrap().instructions.push(i.offset);
        }
        out.push(AnalyzedFunction { func: f.clone(), instructions: insns, blocks });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths() {
        assert_eq!(op_len_6502(0xA9), 2); // LDA #
        assert_eq!(op_len_6502(0xAD), 3); // LDA abs
        assert_eq!(op_len_6502(0xB1), 2); // LDA (zp),Y
        assert_eq!(op_len_6502(0xB9), 3); // LDA abs,Y
        assert_eq!(op_len_6502(0x20), 3); // JSR
        assert_eq!(op_len_6502(0x60), 1); // RTS
        assert_eq!(op_len_6502(0xEA), 1); // NOP
        assert_eq!(op_len_6502(0xD0), 2); // BNE
        assert_eq!(op_len_6502(0x6C), 3); // JMP (ind)
        assert_eq!(op_len_6502(0x0A), 1); // ASL A
        assert_eq!(op_len_6502(0xA2), 2); // LDX #
        assert_eq!(op_len_6502(0x96), 2); // STX zp,Y
        assert_eq!(op_len_6502(0xBE), 3); // LDX abs,Y
    }

    #[test]
    fn blocks_split_at_branches() {
        // C000 LDA $10; CMP #5; BNE C00A; INC $11; RTS; C00A DEC $11; RTS
        let rom = [0xA5, 0x10, 0xC9, 0x05, 0xD0, 0x04, 0xE6, 0x11, 0x60, 0x00, 0xC6, 0x11, 0x60];
        // Note: byte 9 (0x00) is a 1-byte BRK placeholder so the target lands at C00A.
        let f = Function { name: "f".into(), region: "PRG".into(), offset: 0, size: rom.len() as u64, cpu_addr: Some(0xC000) };
        let a = &analyze_6502(&[f], &rom)[0];
        let starts: Vec<u64> = a.blocks.iter().map(|b| b.offset).collect();
        assert_eq!(starts, vec![0, 6, 9, 10]);
    }
}
