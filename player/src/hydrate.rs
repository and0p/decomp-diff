//! Hydration (.agents/CACHE_PLAN.md, "Hydrating"): play the segments whose current content
//! isn't in the local database yet, a slice at a time so a GUI can show it, pause it
//! and stop it. Each finished segment is committed on its own.

use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::collection::{Collection, Status};
use crate::player::{trunk_start_frame, PlayOptions, Recorder, Session};

/// What happened to one segment.
#[derive(Clone, Debug)]
pub struct HydrateEvent {
    pub id: String,
    /// "played" or "stored" (its content was already in the database).
    pub action: &'static str,
    /// Compared with the file's hashes: ok, new, dirty, changed, cleaned or mismatch; when
    /// verifying also recorded and blocked (see `try_record`). "skipped" with "blocked".
    pub verdict: &'static str,
    pub frames: u64,
}

pub struct Hydrator {
    pub targets: Vec<String>,
    /// Index of the segment being worked on.
    pub i: usize,
    pub clean: bool,
    pub force: bool,
    /// Verify: only the segments that aren't clean, recording hashes in memory where it is
    /// safe (`try_record`) instead of writing files. The caller saves.
    pub verify: bool,
    end_now: HashMap<String, String>,
    current: Option<Current>,
    run: Option<i64>,
    pub events: Vec<HydrateEvent>,
    pub frames_played: u64,
}

struct Current {
    rec: Recorder,
    start_frame: u64,
}

impl Hydrator {
    /// `scope`: None for the whole tree, or a segment and whether to include its children.
    pub fn new(col: &Collection, scope: Option<(&str, bool)>, clean: bool, force: bool) -> Result<Hydrator> {
        let order: Vec<String> = col.tree_order().into_iter().filter(|(id, _)| !col.is_draft(id) || scope.is_some_and(|(s, _)| s == *id)).map(|(id, _)| id.to_string()).collect();
        let targets = match scope {
            None => order,
            Some((root, children)) => order.into_iter().filter(|id| id == root || (children && col.chain(id).map(|c| c.contains(&root)).unwrap_or(false))).collect(),
        };
        Ok(Hydrator { targets, i: 0, clean, force, verify: false, end_now: HashMap::new(), current: None, run: None, events: vec![], frames_played: 0 })
    }

    /// Verify all (or a subtree): plays, top down, every segment that isn't clean, and records
    /// hashes in the collection in memory where `try_record` finds it safe. Segments below one
    /// that stays unclean are skipped, since their start can't be trusted.
    pub fn verify(col: &Collection, scope: Option<(&str, bool)>) -> Result<Hydrator> {
        let mut h = Hydrator::new(col, scope, false, false)?;
        h.targets.retain(|id| !col.is_draft(id) && col.tree_status(id) != Status::Clean);
        h.verify = true;
        Ok(h)
    }

    pub fn done(&self) -> bool {
        self.i >= self.targets.len()
    }

    /// The segment being played, and how far: (id, frame, frames).
    pub fn progress(&self) -> Option<(&str, u64, u64)> {
        let c = self.current.as_ref()?;
        Some((c.rec.id.as_str(), c.rec.f, c.rec.frames))
    }

    /// The last video frame shown (when `step` was asked to show frames).
    pub fn last_frame(&mut self) -> Option<crate::libretro::Frame> {
        self.current.as_mut().and_then(|c| c.rec.last_frame.take())
    }

    /// Works until `budget` has passed or everything is done. Returns finished events.
    /// `show_every` > 0 captures every Nth frame's video for display.
    pub fn step(&mut self, s: &mut Session, col: &mut Collection, budget: Duration, show_every: u64) -> Result<Vec<HydrateEvent>> {
        if s.cache.is_none() {
            bail!("hydration needs the local database");
        }
        if self.run.is_none() {
            self.run = Some(s.cache.as_ref().unwrap().begin_run(&col.config.core.build, "hydrate")?);
        }
        let t0 = Instant::now();
        let mut out = vec![];
        let opts = PlayOptions { record: true, ..Default::default() };
        while !self.done() {
            if let Some(cur) = &mut self.current {
                let show = show_every > 0 && cur.rec.f % show_every == 0;
                let finished = s.advance(col, &mut cur.rec, show)?;
                if finished {
                    let cur = self.current.take().unwrap();
                    let start_frame = cur.start_frame;
                    let res = s.finish(col, cur.rec)?;
                    self.frames_played += res.frames;
                    let ev = self.conclude(s, col, &res.id, &res.start_hash, start_frame, &res.end_hash, Some(&res.checkpoints), "played", res.frames)?;
                    out.push(ev);
                    self.i += 1;
                }
            } else {
                let id = self.targets[self.i].clone();
                if self.verify {
                    // Earlier records may have cleaned it (it was only dirty because of its
                    // parent), or its parent may have stayed unclean.
                    let skip = if col.tree_status(&id) == Status::Clean {
                        Some(("stored", "ok"))
                    } else {
                        col.replays[&id].parent().filter(|p| col.tree_status(p) != Status::Clean).map(|_| ("skipped", "blocked"))
                    };
                    if let Some((action, verdict)) = skip {
                        let ev = HydrateEvent { id: id.clone(), action, verdict, frames: 0 };
                        self.events.push(ev.clone());
                        out.push(ev);
                        self.i += 1;
                        continue;
                    }
                }
                let chain: Vec<String> = col.chain(&id)?.into_iter().map(String::from).collect();
                let start_frame = trunk_start_frame(&col.replays[&chain[0]]) + chain[..chain.len() - 1].iter().map(|c| col.replays[c].frames().unwrap_or(0)).sum::<u64>();
                let parent = col.replays[&id].parent().map(String::from);
                // The actual start state's hash; play the parents only if nothing tells us.
                let start = match &parent {
                    None => {
                        s.start_trunk(col, &col.replays[&id])?;
                        s.state_hash(col)?
                    }
                    Some(p) => match self.end_now.get(p) {
                        Some(h) => h.clone(),
                        None => match col.replays[p].get("end-hash").filter(|_| col.tree_status(p) == Status::Clean) {
                            Some(h) => h.to_string(),
                            None => {
                                let res = s.play_chain(col, p, &opts, true)?;
                                for r in &res {
                                    self.end_now.insert(r.id.clone(), r.end_hash.clone());
                                }
                                res.last().ok_or_else(|| anyhow!("internal: empty chain"))?.end_hash.clone()
                            }
                        },
                    },
                };
                let key = s.content_key(col, &id, &start)?;
                let known = if self.force { None } else { s.cache.as_ref().unwrap().content(&key)? };
                if let Some(row) = known {
                    let ev = self.conclude(s, col, &id, &start, start_frame, &row.end_hash, None, "stored", row.frames)?;
                    out.push(ev);
                    self.i += 1;
                } else {
                    match &parent {
                        None => s.start_trunk(col, &col.replays[&id])?,
                        Some(p) => match s.cache.as_ref().unwrap().get_snapshot(&start)? {
                            Some(state) => s.load_state(&state)?,
                            None => {
                                s.play_chain(col, p, &opts, true)?;
                            }
                        },
                    }
                    let rec = s.begin(col, &id, start_frame, &opts)?;
                    if rec.start_hash != start {
                        bail!("{}: started from {} but expected {start}", &id[..8.min(id.len())], rec.start_hash);
                    }
                    self.current = Some(Current { rec, start_frame });
                }
            }
            if t0.elapsed() >= budget {
                break;
            }
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn conclude(&mut self, s: &mut Session, col: &mut Collection, id: &str, start: &str, start_frame: u64, end: &str, cps: Option<&[(u64, String)]>, action: &'static str, frames: u64) -> Result<HydrateEvent> {
        self.end_now.insert(id.to_string(), end.to_string());
        let mut verdict = judge_hashes(col, id, start, end, cps);
        if self.verify {
            verdict = try_record(col, id, start, start_frame, end, cps);
        } else if self.clean && verdict == "dirty" {
            verdict = try_clean(col, id, start, start_frame, end, cps)?;
        }
        let key = s.content_key(col, id, start)?;
        if let (Some(c), Some(run)) = (&s.cache, self.run) {
            c.run_result(run, id, &key, verdict)?;
        }
        let ev = HydrateEvent { id: id.to_string(), action, verdict, frames };
        self.events.push(ev.clone());
        Ok(ev)
    }

    /// Segments that don't match their recorded hashes.
    pub fn problems(&self) -> Vec<&HydrateEvent> {
        self.events.iter().filter(|e| matches!(e.verdict, "dirty" | "changed" | "mismatch" | "blocked")).collect()
    }
}

/// Hashes recorded in a segment's file compared with what playing produced.
pub fn judge_hashes(col: &Collection, id: &str, start: &str, end: &str, checkpoints: Option<&[(u64, String)]>) -> &'static str {
    let r = &col.replays[id];
    if let Some(s) = r.get("start-hash") {
        if s != start {
            return "dirty";
        }
    }
    if let Some(got) = checkpoints {
        for (f, h) in r.checkpoints().unwrap_or_default() {
            if got.iter().any(|(gf, gh)| *gf == f && *gh != h) {
                return "changed";
            }
        }
    }
    match r.get("end-hash") {
        None => "new",
        Some(e) if e == end => "ok",
        Some(_) => "changed",
    }
}

/// "Hydrate and clean": a dirty segment that still ends with its recorded hashes gets
/// its new start hash written, so it is clean again. Otherwise it is a mismatch.
pub fn try_clean(col: &mut Collection, id: &str, start: &str, start_frame: u64, end: &str, checkpoints: Option<&[(u64, String)]>) -> Result<&'static str> {
    let r = &col.replays[id];
    let end_ok = r.get("end-hash") == Some(end);
    let cps_ok = checkpoints.is_none_or(|got| r.checkpoints().unwrap_or_default().iter().all(|(f, h)| got.iter().all(|(gf, gh)| gf != f || gh == h)));
    if end_ok && cps_ok {
        let r = col.replays.get_mut(id).unwrap();
        r.set("start-hash", start);
        r.set("start-frame", &start_frame.to_string());
        col.save_replay(id)?;
        Ok("cleaned")
    } else {
        Ok("mismatch")
    }
}

/// Records what a play of `id` from `start` produced, in memory only, where nothing recorded
/// is overwritten (docs/playback.md, "Recording hashes"):
///
/// - The start must be trusted: the parent is clean and ends with `start`, or for a root
///   segment, `start` is its recorded start hash (if it has one). Otherwise "blocked".
/// - No end hash yet (unverified): start, end hash and start frame are recorded: "recorded".
/// - The recorded end hash and checkpoints still match, but the start hash is old (dirty):
///   the new start hash is recorded: "cleaned". Already matching: "ok".
/// - The end hash or a checkpoint differs: nothing changes, "mismatch". Replacing a recorded
///   hash is a person's call.
pub fn try_record(col: &mut Collection, id: &str, start: &str, start_frame: u64, end: &str, checkpoints: Option<&[(u64, String)]>) -> &'static str {
    let r = &col.replays[id];
    let start_ok = match r.parent() {
        Some(p) => col.tree_status(p) == Status::Clean && col.replays[p].get("end-hash") == Some(start),
        None => r.get("start-hash").is_none_or(|s| s == start),
    };
    if !start_ok {
        return "blocked";
    }
    let cps_ok = checkpoints.is_none_or(|got| r.checkpoints().unwrap_or_default().iter().all(|(f, h)| got.iter().all(|(gf, gh)| gf != f || gh == h)));
    if !cps_ok {
        return "mismatch";
    }
    let verdict = match r.get("end-hash") {
        Some(e) if e != end => return "mismatch",
        None => "recorded",
        Some(_) if r.get("start-hash") == Some(start) => return "ok",
        Some(_) => "cleaned",
    };
    let r = col.replays.get_mut(id).unwrap();
    r.set("start-hash", start);
    r.set("end-hash", end);
    r.set("start-frame", &start_frame.to_string());
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Replay;

    fn project(name: &str) -> Collection {
        let dir = std::env::temp_dir().join(format!("dj-hydrate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("segments")).unwrap();
        std::fs::write(dir.join("config.toml"), "format = 1\nname = \"t\"\n[rom]\nsha256 = \"\"\nfile = \"t.nes\"\n[core]\nname = \"c\"\nbuild = \"c\"\nsha256 = \"\"\n[[ports]]\nport = 1\ndevice = \"JOYPAD\"\n[hash]\nalgorithm = \"blake3\"\nregions = [\"RAM\"]\n").unwrap();
        let mut col = Collection::open(&dir).unwrap();
        let mut a = Replay::new("a", None, "root", 10);
        a.set("end-hash", "A1");
        col.replays.insert("a".into(), a);
        let mut b = Replay::new("b", Some("a"), "child", 10);
        b.set("start-hash", "A0");
        b.set("end-hash", "B");
        col.replays.insert("b".into(), b);
        col
    }

    #[test]
    fn dirty_segment_that_still_ends_the_same_is_cleaned() {
        let mut col = project("clean");
        assert_eq!(col.tree_status("b"), Status::Dirty);
        assert_eq!(try_record(&mut col, "b", "A1", 10, "B", None), "cleaned");
        assert_eq!(col.tree_status("b"), Status::Clean);
    }

    #[test]
    fn a_different_end_is_never_overwritten() {
        let mut col = project("mismatch");
        assert_eq!(try_record(&mut col, "b", "A1", 10, "X", None), "mismatch");
        assert_eq!(col.replays["b"].get("end-hash"), Some("B"));
    }

    #[test]
    fn unverified_is_recorded_only_from_a_trusted_start() {
        let mut col = project("record");
        col.replays.get_mut("b").unwrap().remove_all("end-hash");
        // Not where the parent ends: blocked.
        assert_eq!(try_record(&mut col, "b", "A0", 10, "B2", None), "blocked");
        assert_eq!(try_record(&mut col, "b", "A1", 10, "B2", None), "recorded");
        assert_eq!(col.replays["b"].get("end-hash"), Some("B2"));
        assert_eq!(col.tree_status("b"), Status::Clean);
        // A parent that isn't clean blocks its children.
        col.replays.get_mut("a").unwrap().remove_all("end-hash");
        assert_eq!(try_record(&mut col, "b", "A1", 10, "B2", None), "blocked");
    }

    #[test]
    fn verify_targets_only_unclean_segments() {
        let col = project("targets");
        let h = Hydrator::verify(&col, None).unwrap();
        assert_eq!(h.targets, vec!["b".to_string()]);
    }
}
