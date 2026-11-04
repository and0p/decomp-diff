//! Parser and writer for `.replay` files (SPEC.md section 3).

use anyhow::{anyhow, bail, Context, Result};
use std::fmt::Write as _;
use std::path::Path;

pub const MAGIC: &str = "emucov-replay 1";

// libretro device ids (libretro.h).
pub const DEVICE_NONE: u32 = 0;
pub const DEVICE_JOYPAD: u32 = 1;
pub const DEVICE_MOUSE: u32 = 2;
pub const DEVICE_KEYBOARD: u32 = 3;
pub const DEVICE_LIGHTGUN: u32 = 4;
pub const DEVICE_ANALOG: u32 = 5;
pub const DEVICE_POINTER: u32 = 6;

/// One input value as libretro addresses it: `input_state(port, device, index, id)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InputKey {
    pub port: u32,
    pub device: u32,
    pub index: u32,
    pub id: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Sets a held value. `impulse` values last one frame only (mouse motion).
    Input { key: InputKey, value: i16, impulse: bool },
    Device { port: u32, device: u32 },
    Write { region: String, address: u64, bytes: Vec<u8> },
    Event { text: String, pos: Option<(i32, i32)> },
    Reset,
    Screenshot,
    Rng { name: String, count: u64 },
}

#[derive(Clone, Debug)]
pub struct Line {
    pub frame: u64,
    pub command: Command,
    pub line_no: usize,
}

#[derive(Clone, Debug)]
enum HeaderLine {
    Field(String, String),
    Other(String),
}

#[derive(Clone, Debug)]
pub struct Replay {
    header: Vec<HeaderLine>,
    body_text: Vec<String>,
    pub lines: Vec<Line>,
}

impl Replay {
    pub fn new(id: &str, parent: Option<&str>, title: &str, frames: u64) -> Replay {
        let mut r = Replay { header: vec![], body_text: vec![], lines: vec![] };
        r.set("id", id);
        r.set("parent", parent.unwrap_or("none"));
        if !title.is_empty() {
            r.set("title", title);
        }
        r.set("frames", &frames.to_string());
        r
    }

    pub fn load(path: &Path) -> Result<Replay> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Replay::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Replay> {
        let mut it = text.lines().enumerate();
        let first = it.next().map(|(_, l)| l.trim()).unwrap_or("");
        if first != MAGIC {
            bail!("first line must be `{MAGIC}`");
        }
        let mut header = vec![];
        let mut in_body = false;
        let mut body_text = vec![];
        let mut lines = vec![];
        let mut last_frame = 0u64;
        for (n, raw) in it {
            let line_no = n + 1;
            let l = raw.trim_end_matches('\r');
            if !in_body {
                if l.trim() == "---" {
                    in_body = true;
                    continue;
                }
                let t = l.trim();
                if t.is_empty() || t.starts_with('#') {
                    header.push(HeaderLine::Other(l.to_string()));
                    continue;
                }
                let (k, v) = t.split_once(':').ok_or_else(|| anyhow!("line {line_no}: expected `key: value`"))?;
                header.push(HeaderLine::Field(k.trim().to_string(), v.trim().to_string()));
            } else {
                body_text.push(l.to_string());
                let t = l.trim();
                if t.is_empty() || t.starts_with('#') {
                    continue;
                }
                let (frame, command) = parse_body_line(t).with_context(|| format!("line {line_no}: `{t}`"))?;
                if frame < last_frame {
                    bail!("line {line_no}: frame {frame} comes after frame {last_frame}");
                }
                last_frame = frame;
                lines.push(Line { frame, command, line_no });
            }
        }
        let r = Replay { header, body_text, lines };
        r.id()?;
        r.frames()?;
        if r.get("parent").is_none() {
            bail!("missing `parent` (use `parent: none` for a trunk)");
        }
        if let Some(last) = r.lines.last() {
            if last.frame >= r.frames()? {
                bail!("line {}: frame {} is past the end (frames: {})", last.line_no, last.frame, r.frames()?);
            }
        }
        Ok(r)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.header.iter().find_map(|h| match h {
            HeaderLine::Field(k, v) if k == key => Some(v.as_str()),
            _ => None,
        })
    }

    pub fn get_all(&self, key: &str) -> Vec<&str> {
        self.header
            .iter()
            .filter_map(|h| match h {
                HeaderLine::Field(k, v) if k == key => Some(v.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Sets a header field, keeping its position if it exists.
    pub fn set(&mut self, key: &str, value: &str) {
        for h in self.header.iter_mut() {
            if let HeaderLine::Field(k, v) = h {
                if k == key {
                    *v = value.to_string();
                    return;
                }
            }
        }
        self.header.push(HeaderLine::Field(key.to_string(), value.to_string()));
    }

    pub fn remove_all(&mut self, key: &str) {
        self.header.retain(|h| !matches!(h, HeaderLine::Field(k, _) if k == key));
    }

    pub fn add(&mut self, key: &str, value: &str) {
        self.header.push(HeaderLine::Field(key.to_string(), value.to_string()));
    }

    pub fn id(&self) -> Result<&str> {
        self.get("id").ok_or_else(|| anyhow!("missing `id`"))
    }

    pub fn parent(&self) -> Option<&str> {
        match self.get("parent") {
            None | Some("none") | Some("") => None,
            Some(p) => Some(p),
        }
    }

    pub fn title(&self) -> &str {
        self.get("title").unwrap_or("")
    }

    pub fn frames(&self) -> Result<u64> {
        let f = self.get("frames").ok_or_else(|| anyhow!("missing `frames`"))?;
        f.parse().map_err(|_| anyhow!("`frames` is not a number: {f}"))
    }

    pub fn checkpoints(&self) -> Result<Vec<(u64, String)>> {
        self.get_all("checkpoint")
            .into_iter()
            .map(|v| {
                let (f, h) = v.split_once(char::is_whitespace).ok_or_else(|| anyhow!("checkpoint needs `<frame> <hash>`"))?;
                Ok((f.trim().parse()?, h.trim().to_string()))
            })
            .collect()
    }

    pub fn is_poked(&self) -> bool {
        self.lines.iter().any(|l| matches!(l.command, Command::Write { .. }))
    }

    pub fn to_text(&self) -> String {
        let mut s = String::new();
        s.push_str(MAGIC);
        s.push('\n');
        for h in &self.header {
            match h {
                HeaderLine::Field(k, v) => {
                    let _ = writeln!(s, "{k}: {v}");
                }
                HeaderLine::Other(o) => {
                    s.push_str(o);
                    s.push('\n');
                }
            }
        }
        s.push_str("---\n");
        for b in &self.body_text {
            s.push_str(b);
            s.push('\n');
        }
        s
    }

    /// Appends body lines (already formatted, frame first).
    #[allow(dead_code)]
    pub fn push_body(&mut self, text: &str) -> Result<()> {
        let (frame, command) = parse_body_line(text.trim())?;
        self.lines.push(Line { frame, command, line_no: 0 });
        self.body_text.push(text.to_string());
        Ok(())
    }
}

fn parse_num(s: &str) -> Result<u64> {
    let s = s.trim();
    let (digits, radix) = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => (h, 16),
        None => (s, 10),
    };
    u64::from_str_radix(digits, radix).map_err(|_| anyhow!("not a number: {s}"))
}

fn parse_hex(s: &str) -> Result<u64> {
    let s = s.trim();
    let h = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u64::from_str_radix(h, 16).map_err(|_| anyhow!("not a hex number: {s}"))
}

fn parse_value(s: &str) -> Result<i16> {
    let v: i64 = if let Some(h) = s.strip_prefix("0x") { i64::from_str_radix(h, 16)? } else { s.parse()? };
    if !(i16::MIN as i64..=i16::MAX as i64).contains(&v) {
        bail!("value {v} is outside -32768..32767");
    }
    Ok(v as i16)
}

pub fn parse_body_line(t: &str) -> Result<(u64, Command)> {
    let (frame_s, rest) = t.split_once(char::is_whitespace).ok_or_else(|| anyhow!("expected `<frame> <command>`"))?;
    let frame = parse_num(frame_s)?;
    let rest = rest.trim();
    if let Some(after) = rest.strip_prefix('[') {
        let (tag, args) = after.split_once(']').ok_or_else(|| anyhow!("unclosed `[`"))?;
        let args = args.trim();
        let mut tag_words = tag.split_whitespace();
        let name = tag_words.next().unwrap_or("").to_ascii_uppercase();
        let cmd = match name.as_str() {
            "WRITE" => {
                let mut w = args.split_whitespace();
                let region = w.next().ok_or_else(|| anyhow!("[WRITE] needs a region"))?.to_ascii_uppercase();
                let address = parse_hex(w.next().ok_or_else(|| anyhow!("[WRITE] needs an address"))?)?;
                let bytes = w.map(|b| parse_hex(b).and_then(|v| u8::try_from(v).map_err(|_| anyhow!("byte {b} > 0xFF")))).collect::<Result<Vec<u8>>>()?;
                if bytes.is_empty() {
                    bail!("[WRITE] needs at least one byte");
                }
                Command::Write { region, address, bytes }
            }
            "EVENT" => {
                let pos = match tag_words.next() {
                    Some(p) => {
                        let p = p.strip_prefix('@').ok_or_else(|| anyhow!("event position must look like @x,y"))?;
                        let (x, y) = p.split_once(',').ok_or_else(|| anyhow!("event position must look like @x,y"))?;
                        Some((x.parse()?, y.parse()?))
                    }
                    None => None,
                };
                Command::Event { text: args.to_string(), pos }
            }
            "DEVICE" => {
                let mut w = args.split_whitespace();
                let port = parse_port(w.next().ok_or_else(|| anyhow!("[DEVICE] needs a port"))?)?;
                let dev = w.next().ok_or_else(|| anyhow!("[DEVICE] needs a device"))?;
                Command::Device { port, device: device_from_name(dev)? }
            }
            "RESET" => Command::Reset,
            "SCREENSHOT" => Command::Screenshot,
            "RNG" => {
                let mut w = args.split_whitespace();
                let name = w.next().ok_or_else(|| anyhow!("[RNG] needs a name"))?.to_string();
                let count = parse_num(w.next().unwrap_or("1"))?;
                Command::Rng { name, count }
            }
            other => bail!("unknown command [{other}]"),
        };
        return Ok((frame, cmd));
    }
    Ok((frame, parse_input(rest)?))
}

fn parse_port(s: &str) -> Result<u32> {
    let n = s.strip_prefix('P').or_else(|| s.strip_prefix('p')).ok_or_else(|| anyhow!("port must look like P1"))?;
    let n: u32 = n.parse()?;
    if n == 0 {
        bail!("ports start at P1");
    }
    Ok(n - 1)
}

pub fn device_from_name(s: &str) -> Result<u32> {
    Ok(match s.to_ascii_uppercase().as_str() {
        "NONE" => DEVICE_NONE,
        "JOYPAD" => DEVICE_JOYPAD,
        "MOUSE" => DEVICE_MOUSE,
        "KEYBOARD" => DEVICE_KEYBOARD,
        "LIGHTGUN" => DEVICE_LIGHTGUN,
        "ANALOG" => DEVICE_ANALOG,
        "POINTER" => DEVICE_POINTER,
        other => other.parse().map_err(|_| anyhow!("unknown device {other}"))?,
    })
}

const JOYPAD: [&str; 16] = ["B", "Y", "SELECT", "START", "UP", "DOWN", "LEFT", "RIGHT", "A", "X", "L", "R", "L2", "R2", "L3", "R3"];

fn joypad_id(s: &str) -> Option<u32> {
    JOYPAD.iter().position(|b| *b == s).map(|i| i as u32)
}

fn lightgun_id(s: &str) -> Option<u32> {
    Some(match s {
        "X" => 0,
        "Y" => 1,
        "TRIGGER" => 2,
        "AUX_A" => 3,
        "AUX_B" => 4,
        "START" => 6,
        "SELECT" => 7,
        "AUX_C" => 8,
        "DPAD_UP" => 9,
        "DPAD_DOWN" => 10,
        "DPAD_LEFT" => 11,
        "DPAD_RIGHT" => 12,
        "SCREEN_X" => 13,
        "SCREEN_Y" => 14,
        "IS_OFFSCREEN" => 15,
        "RELOAD" => 16,
        _ => return None,
    })
}

fn mouse_id(s: &str) -> Option<(u32, bool)> {
    Some(match s {
        "X" => (0, true),
        "Y" => (1, true),
        "LEFT" => (2, false),
        "RIGHT" => (3, false),
        "WHEELUP" => (4, true),
        "WHEELDOWN" => (5, true),
        "MIDDLE" => (6, false),
        "HORIZ_WHEELUP" => (7, true),
        "HORIZ_WHEELDOWN" => (8, true),
        "BUTTON_4" => (9, false),
        "BUTTON_5" => (10, false),
        _ => return None,
    })
}

fn pointer_id(s: &str) -> Option<u32> {
    Some(match s {
        "X" => 0,
        "Y" => 1,
        "PRESSED" => 2,
        "COUNT" => 3,
        "IS_OFFSCREEN" => 15,
        _ => return None,
    })
}

/// RETROK_ key codes for the keys replays are most likely to use.
fn key_id(s: &str) -> Option<u32> {
    if s.len() == 1 {
        let c = s.as_bytes()[0];
        if c.is_ascii_uppercase() {
            return Some((c.to_ascii_lowercase()) as u32);
        }
        if c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    if let Some(n) = s.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=15).contains(&n) {
            return Some(281 + n);
        }
    }
    Some(match s {
        "BACKSPACE" => 8,
        "TAB" => 9,
        "RETURN" => 13,
        "ESCAPE" => 27,
        "SPACE" => 32,
        "DELETE" => 127,
        "UP" => 273,
        "DOWN" => 274,
        "RIGHT" => 275,
        "LEFT" => 276,
        "INSERT" => 277,
        "HOME" => 278,
        "END" => 279,
        "PAGEUP" => 280,
        "PAGEDOWN" => 281,
        "RSHIFT" => 303,
        "LSHIFT" => 304,
        "RCTRL" => 305,
        "LCTRL" => 306,
        "RALT" => 307,
        "LALT" => 308,
        _ => return None,
    })
}

fn parse_input(rest: &str) -> Result<Command> {
    let w: Vec<&str> = rest.split_whitespace().collect();
    if w.len() < 3 {
        bail!("input lines look like `P1 JOYPAD A 1`");
    }
    let port = parse_port(w[0])?;
    let dev = w[1].to_ascii_uppercase();
    let up = |i: usize| w.get(i).map(|s| s.to_ascii_uppercase()).ok_or_else(|| anyhow!("missing field"));
    let val = |i: usize| parse_value(w.get(i).ok_or_else(|| anyhow!("missing value"))?);
    let (device, index, id, value, impulse) = match dev.as_str() {
        "JOYPAD" => {
            let b = up(2)?;
            (DEVICE_JOYPAD, 0, joypad_id(&b).ok_or_else(|| anyhow!("unknown joypad button {b}"))?, val(3)?, false)
        }
        "ANALOG" => {
            let stick = up(2)?;
            match stick.as_str() {
                "LEFT" | "RIGHT" => {
                    let axis = up(3)?;
                    let id = match axis.as_str() {
                        "X" => 0,
                        "Y" => 1,
                        _ => bail!("analog axis must be X or Y"),
                    };
                    (DEVICE_ANALOG, if stick == "LEFT" { 0 } else { 1 }, id, val(4)?, false)
                }
                "BUTTON" => {
                    let b = up(3)?;
                    (DEVICE_ANALOG, 2, joypad_id(&b).ok_or_else(|| anyhow!("unknown button {b}"))?, val(4)?, false)
                }
                _ => bail!("analog control must be LEFT, RIGHT or BUTTON"),
            }
        }
        "LIGHTGUN" => {
            let c = up(2)?;
            let id = lightgun_id(&c).ok_or_else(|| anyhow!("unknown lightgun control {c}"))?;
            // X/Y (ids 0,1) are deprecated relative motion: impulses.
            (DEVICE_LIGHTGUN, 0, id, val(3)?, id <= 1)
        }
        "MOUSE" => {
            let c = up(2)?;
            let (id, impulse) = mouse_id(&c).ok_or_else(|| anyhow!("unknown mouse control {c}"))?;
            (DEVICE_MOUSE, 0, id, val(3)?, impulse)
        }
        "POINTER" => {
            // P1 POINTER X 0  or  P1 POINTER 1 X 0
            let (index, at) = match w.get(2).and_then(|s| s.parse::<u32>().ok()) {
                Some(i) => (i, 3),
                None => (0, 2),
            };
            let c = up(at)?;
            (DEVICE_POINTER, index, pointer_id(&c).ok_or_else(|| anyhow!("unknown pointer control {c}"))?, val(at + 1)?, false)
        }
        "KEYBOARD" => {
            let k = up(2)?;
            (DEVICE_KEYBOARD, 0, key_id(&k).ok_or_else(|| anyhow!("unknown key {k}; use RAW for others"))?, val(3)?, false)
        }
        "RAW" => {
            if w.len() < 6 {
                bail!("RAW lines look like `P1 RAW <device> <index> <id> <value>`");
            }
            (parse_num(w[2])? as u32, parse_num(w[3])? as u32, parse_num(w[4])? as u32, val(5)?, false)
        }
        other => bail!("unknown device {other}"),
    };
    Ok(Command::Input { key: InputKey { port, device, index, id }, value, impulse })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "emucov-replay 1
id: 3b8f0f2c-1d44-4c1e-9e55-0d7a3c1b2e90
parent: none
title: Boot
frames: 100
# a comment
---
0 P1 JOYPAD START 1
4 P1 JOYPAD START 0
5 P2 MOUSE X -3
6 P1 ANALOG LEFT X -32768
7 [WRITE] RAM 0x12 13 ff
8 [EVENT @10,20] Title visible
9 [DEVICE] P2 MOUSE
10 P1 RAW 1 0 8 1
";

    #[test]
    fn parses_and_round_trips() {
        let r = Replay::parse(SAMPLE).unwrap();
        assert_eq!(r.id().unwrap(), "3b8f0f2c-1d44-4c1e-9e55-0d7a3c1b2e90");
        assert_eq!(r.parent(), None);
        assert_eq!(r.frames().unwrap(), 100);
        assert_eq!(r.lines.len(), 8);
        assert!(r.is_poked());
        assert_eq!(
            r.lines[0].command,
            Command::Input { key: InputKey { port: 0, device: 1, index: 0, id: 3 }, value: 1, impulse: false }
        );
        assert_eq!(
            r.lines[2].command,
            Command::Input { key: InputKey { port: 1, device: 2, index: 0, id: 0 }, value: -3, impulse: true }
        );
        assert_eq!(r.lines[4].command, Command::Write { region: "RAM".into(), address: 0x12, bytes: vec![0x13, 0xff] });
        assert_eq!(r.lines[5].command, Command::Event { text: "Title visible".into(), pos: Some((10, 20)) });
        assert_eq!(Replay::parse(&r.to_text()).unwrap().to_text(), r.to_text());
    }

    #[test]
    fn rejects_bad_files() {
        assert!(Replay::parse("nope").is_err());
        let past_end = SAMPLE.replace("frames: 100", "frames: 5");
        assert!(Replay::parse(&past_end).is_err());
        let unordered = SAMPLE.replace("4 P1 JOYPAD START 0", "0 P1 JOYPAD START 0").replace("0 P1 JOYPAD START 1", "3 P1 JOYPAD START 1");
        assert!(Replay::parse(&unordered).is_err());
        let unknown = SAMPLE.replace("[RESETX]", "").replace("9 [DEVICE] P2 MOUSE", "9 [TELEPORT] P2");
        assert!(Replay::parse(&unknown).is_err());
    }
}
