//! emucov-gui: browse a replay collection, play replays with live video, and
//! edit button inputs on a piano roll.
//!
//! Layout: menu bar on top, replay tree on the left, emulator preview in the
//! middle, piano roll along the bottom, status line at the very bottom.

use anyhow::{anyhow, Result};
use eframe::egui::{self, Color32, RichText};
use emucov::collection::{Collection, LocalConfig, Status};
use emucov::format::{Command, InputKey};
use emucov::player::{trunk_start_frame, Session};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

mod roll;

/// A save state is kept every this many frames, so seeking never replays more than this.
const SNAP_EVERY: u64 = 10;

fn main() -> eframe::Result {
    let start = std::env::args().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 860.0]).with_min_inner_size([800.0, 560.0]).with_title("emucov"),
        ..Default::default()
    };
    eframe::run_native(
        "emucov",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_theme(egui::Theme::Dark);
            CTX.with(|c| *c.borrow_mut() = Some(cc.egui_ctx.clone()));
            let mut app = App::default();
            if let Some(p) = start {
                app.open_collection(&p);
            }
            Ok(Box::new(app))
        }),
    )
}

/// Where playback stands inside the selected replay.
struct Playback {
    id: String,
    /// Frame of power-on at which this replay starts.
    start_frame: u64,
    frames: u64,
    /// Frames of this replay run so far. The picture shown is frame `ran - 1`.
    ran: u64,
    /// Save states at the start of frame N (before frame N's lines apply).
    snapshots: BTreeMap<u64, Vec<u8>>,
    checkpoints: BTreeMap<u64, String>,
    /// First checkpoint whose hash did not match, if any.
    checkpoint_miss: Option<u64>,
    /// State hash after the last frame, once playback has reached it.
    end_hash: Option<String>,
    /// State hash at frame 0, before any of this replay's lines.
    start_hash: Option<String>,
}

impl Playback {
    fn shown(&self) -> Option<u64> {
        self.ran.checked_sub(1)
    }
}

struct App {
    col: Option<Collection>,
    session: Option<Session>,
    core_error: Option<String>,
    fps: f64,
    aspect: f32,
    selected: Option<String>,
    pb: Option<Playback>,
    playing: bool,
    clock: f64,
    /// Long emulation work, run a slice per screen refresh so the window stays live.
    job: Option<Job>,
    texture: Option<egui::TextureHandle>,
    tree_closed: HashSet<String>,
    /// Tree filter text: matches title, id or tags.
    filter: String,
    /// The node the tree is focused on (hoisted): only it and its descendants show.
    hoist: Option<String>,
    edited: BTreeSet<String>,
    roll: roll::RollState,
    message: Option<(String, bool)>,
    /// Frames per tick multiplier; 0 means as fast as possible.
    speed: u32,
    /// At the end of a node with exactly one child, carry on into it.
    auto_continue: bool,
    /// "Play to": nodes still to play through after the current one, and the last of them.
    route: Vec<String>,
    route_target: Option<String>,
    node_dialog: Option<NodeDialog>,
    /// Inline rename in the tree: node id, text, and whether focus is still to be given.
    renaming: Option<(String, String, bool)>,
    /// The node waiting for delete confirmation.
    delete_dialog: Option<String>,
    undo: Vec<UndoStep>,
    redo: Vec<UndoStep>,
    clip: Option<Clip>,
    /// Text to put on the system clipboard at the end of this frame.
    ctx_copy: Option<String>,
    frame_dialog: Option<FrameDialog>,
}

/// Work too long to do in one screen refresh.
enum Job {
    /// Run the selected node forward until `to` frames have run.
    Seek { from: u64, to: u64 },
    /// Play ancestors (`queue`) without a cached state to reach the start of `id`.
    Enter { id: String, queue: Vec<String>, qi: usize, frame: u64, start_frame: u64, done: u64, total: u64, then_seek: Option<u64> },
}

/// Time spent on a job per screen refresh.
const JOB_SLICE: std::time::Duration = std::time::Duration::from_millis(25);

/// One step of undo history: the affected nodes as they were before the change
/// (None: the node did not exist), and where the user was.
struct UndoStep {
    label: String,
    nodes: Vec<(String, Option<emucov::format::Replay>)>,
    selected: Option<String>,
    frame: Option<u64>,
}

const UNDO_LIMIT: usize = 200;

/// Copied notes: button, start relative to the leftmost copied note, and length.
struct Clip {
    notes: Vec<(InputKey, u64, u64)>,
}

struct NodeDialog {
    id: String,
    title: String,
    frames: u64,
}

struct FrameDialog {
    frame: u64,
    /// (line number, command text) for the non-input lines on this frame.
    rows: Vec<(usize, String)>,
    new_text: String,
    /// Put the keyboard focus in the new-line field on the next draw.
    focus: bool,
}

impl Default for App {
    fn default() -> Self {
        App {
            col: None,
            session: None,
            core_error: None,
            fps: 60.0,
            aspect: 0.0,
            selected: None,
            pb: None,
            playing: false,
            clock: 0.0,
            job: None,
            texture: None,
            tree_closed: HashSet::new(),
            filter: String::new(),
            hoist: None,
            edited: BTreeSet::new(),
            roll: roll::RollState::default(),
            message: None,
            speed: 1,
            auto_continue: true,
            route: vec![],
            route_target: None,
            node_dialog: None,
            renaming: None,
            delete_dialog: None,
            undo: vec![],
            redo: vec![],
            clip: None,
            ctx_copy: None,
            frame_dialog: None,
        }
    }
}

fn status_color(s: Status) -> Color32 {
    match s {
        Status::Clean => Color32::from_rgb(92, 184, 92),
        Status::Dirty => Color32::from_rgb(240, 160, 60),
        Status::Unverified => Color32::from_gray(130),
    }
}

fn col_title(col: Option<&Collection>, id: &str) -> String {
    col.and_then(|c| c.replays.get(id)).map_or(String::new(), |r| r.title().to_string())
}

fn status_dot(ui: &mut egui::Ui, s: Status) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, status_color(s));
    resp
}

/// A small icon button in the style of VS Code's tree toolbar: two stacked boxes
/// with a minus (collapse all) or a plus (expand all).
fn fold_button(ui: &mut egui::Ui, collapse: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let c = if resp.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
    let stroke = egui::Stroke::new(1.2, c);
    let back = egui::Rect::from_min_size(rect.min + egui::vec2(7.0, 4.0), egui::vec2(9.0, 9.0));
    let front = egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 7.0), egui::vec2(9.0, 9.0));
    // The back box shows only as an L above and to the right of the front one.
    p.line_segment([back.left_top(), back.right_top()], stroke);
    p.line_segment([back.right_top(), back.right_bottom()], stroke);
    p.rect_filled(front, 1.0, ui.visuals().panel_fill);
    p.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Inside);
    let m = front.center();
    p.line_segment([m - egui::vec2(2.5, 0.0), m + egui::vec2(2.5, 0.0)], stroke);
    if !collapse {
        p.line_segment([m - egui::vec2(0.0, 2.5), m + egui::vec2(0.0, 2.5)], stroke);
    }
    resp
}

/// `id` and every node below it.
fn subtree(col: &Collection, id: &str) -> Vec<String> {
    let mut out = vec![id.to_string()];
    let mut i = 0;
    while i < out.len() {
        let kids: Vec<String> = col.children(Some(&out[i])).into_iter().map(String::from).collect();
        out.extend(kids);
        i += 1;
    }
    out
}

/// The first frame where two versions of a node's lines differ, if they do.
fn first_change(a: &emucov::format::Replay, b: &emucov::format::Replay) -> Option<u64> {
    let n = a.lines.len().min(b.lines.len());
    for i in 0..n {
        let (x, y) = (&a.lines[i], &b.lines[i]);
        if x.frame != y.frame || x.command != y.command {
            return Some(x.frame.min(y.frame));
        }
    }
    match (a.lines.get(n), b.lines.get(n)) {
        (Some(l), _) | (_, Some(l)) => Some(l.frame),
        _ => None,
    }
}

fn abs_path(root: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    }
}

impl App {
    fn info(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), false));
    }

    fn error(&mut self, e: impl std::fmt::Display) {
        self.message = Some((format!("{e:#}"), true));
    }

    // ---- collection and core ----

    fn open_collection(&mut self, dir: &Path) {
        match Collection::open(dir) {
            Ok(col) => {
                // Only one libretro core can be loaded per process, so drop the old one first.
                self.session = None;
                self.pb = None;
                self.playing = false;
                self.texture = None;
                self.edited.clear();
                self.selected = None;
                self.hoist = None;
                self.undo.clear();
                self.redo.clear();
                self.roll.forget();
                let first = col.tree_order().first().map(|(id, _)| id.to_string());
                let n = col.replays.len();
                self.col = Some(col);
                self.open_session();
                self.info(format!("Opened {} ({n} replays)", dir.display()));
                if let Some(id) = first {
                    self.select(&id);
                }
            }
            Err(e) => self.error(format!("Could not open {}: {e:#}", dir.display())),
        }
    }

    fn open_session(&mut self) {
        let Some(col) = &self.col else { return };
        self.session = None;
        let local = col.load_local().unwrap_or_default();
        let (rom, core) = match (&local.rom, &local.core) {
            (Some(r), Some(c)) => (abs_path(&col.root, r), abs_path(&col.root, c)),
            (None, _) => {
                self.core_error = Some("No ROM set for this collection on this machine. Use Emulation → Set ROM…".into());
                return;
            }
            (_, None) => {
                self.core_error = Some("No libretro core set for this collection on this machine. Use Emulation → Set core…".into());
                return;
            }
        };
        match Session::open(col, &rom, &core) {
            Ok(s) => {
                let (fps, aspect) = s.core.timing();
                self.fps = if fps > 1.0 { fps } else { 60.0 };
                self.aspect = aspect;
                self.session = Some(s);
                self.core_error = None;
            }
            Err(e) => self.core_error = Some(format!("Could not start the core: {e:#}")),
        }
    }

    fn set_local(&mut self, rom: Option<PathBuf>, core: Option<PathBuf>) {
        let Some(col) = &self.col else { return };
        let mut local: LocalConfig = col.load_local().unwrap_or_default();
        if rom.is_some() {
            local.rom = rom;
        }
        if core.is_some() {
            local.core = core;
        }
        if let Err(e) = col.save_local(&local) {
            self.error(e);
            return;
        }
        self.pb = None;
        self.open_session();
        if let Some(id) = self.selected.clone() {
            self.select(&id);
        }
    }

    fn save(&mut self) {
        let Some(col) = &mut self.col else { return };
        let ids: Vec<String> = self.edited.iter().cloned().collect();
        for id in &ids {
            // A node removed by undo (a split or a new node) loses its file.
            let res = if col.replays.contains_key(id) { col.save_replay(id) } else { col.delete_replay_file(id) };
            if let Err(e) = res {
                self.message = Some((format!("Saving {id}: {e:#}"), true));
                return;
            }
        }
        self.edited.clear();
        self.info(format!("Saved {} replay(s)", ids.len()));
    }

    fn reload(&mut self) {
        if let Some(root) = self.col.as_ref().map(|c| c.root.clone()) {
            let sel = self.selected.clone();
            self.open_collection(&root);
            if let Some(id) = sel {
                if self.col.as_ref().is_some_and(|c| c.replays.contains_key(&id)) {
                    self.select(&id);
                }
            }
        }
    }

    // ---- playback ----

    fn select(&mut self, id: &str) {
        self.playing = false;
        self.route.clear();
        self.route_target = None;
        self.job = None;
        self.selected = Some(id.to_string());
        self.pb = None;
        if let Err(e) = self.try_select(id) {
            self.error(format!("Could not reach the start of this replay: {e:#}"));
        }
    }

    fn try_select(&mut self, id: &str) -> Result<()> {
        let (Some(col), Some(s)) = (&self.col, &mut self.session) else { return Ok(()) };
        let chain: Vec<String> = col.chain(id)?.into_iter().map(String::from).collect();
        let n = chain.len();
        let fast = n == 1 || s.load_cached_state(col, &chain[n - 2])?.is_some();
        if fast {
            let start_frame = s.enter_replay(col, id)?;
            self.begin_playback(id, start_frame)?;
            self.roll.follow_to(0);
            self.seek(0);
            return Ok(());
        }
        // No cached start state: play the ancestors a slice at a time, from the
        // deepest one that has a cached end state, or from power-on.
        let mut first = 0;
        for i in (0..n - 1).rev() {
            if let Some(state) = s.load_cached_state(col, &chain[i])? {
                s.load_state(&state)?;
                first = i + 1;
                break;
            }
        }
        if first == 0 {
            s.start_trunk(col, &col.replays[&chain[0]])?;
        }
        let frames = |c: &String| col.replays[c].frames().unwrap_or(0);
        let queue: Vec<String> = chain[first..n - 1].to_vec();
        let total = queue.iter().map(frames).sum();
        let start_frame = trunk_start_frame(&col.replays[&chain[0]]) + chain[..n - 1].iter().map(frames).sum::<u64>();
        self.job = Some(Job::Enter { id: id.to_string(), queue, qi: 0, frame: 0, start_frame, done: 0, total, then_seek: None });
        Ok(())
    }

    /// The core is at frame 0 of `id`: set up playback there.
    fn begin_playback(&mut self, id: &str, start_frame: u64) -> Result<()> {
        let (Some(col), Some(s)) = (&self.col, &mut self.session) else { return Ok(()) };
        let r = &col.replays[id];
        let frames = r.frames().unwrap_or(0);
        let checkpoints = r.checkpoints().unwrap_or_default().into_iter().collect();
        let snap = s.snapshot()?;
        let start_hash = s.state_hash(col)?;
        self.pb = Some(Playback { id: id.to_string(), start_frame, frames, ran: 0, snapshots: BTreeMap::from([(0, snap)]), checkpoints, checkpoint_miss: None, end_hash: None, start_hash: Some(start_hash) });
        Ok(())
    }

    /// Does one slice of the current job.
    fn work(&mut self) {
        let Some(job) = self.job.take() else { return };
        if let Err(e) = self.try_work(job) {
            self.job = None;
            self.playing = false;
            self.error(e);
        }
    }

    fn try_work(&mut self, job: Job) -> Result<()> {
        let t0 = std::time::Instant::now();
        match job {
            Job::Seek { from, to } => loop {
                let ran = self.pb.as_ref().map_or(to, |p| p.ran);
                if ran >= to {
                    return Ok(());
                }
                let last = ran + 1 == to;
                self.run_one(last)?;
                if !last && t0.elapsed() > JOB_SLICE {
                    self.job = Some(Job::Seek { from, to });
                    return Ok(());
                }
            },
            Job::Enter { id, queue, mut qi, mut frame, start_frame, mut done, total, then_seek } => {
                loop {
                    let (Some(col), Some(s)) = (&self.col, &mut self.session) else { return Ok(()) };
                    if qi == queue.len() {
                        s.restore_inputs(col, &id, 0)?;
                        s.core.coverage_clear();
                        self.begin_playback(&id, start_frame)?;
                        self.roll.follow_to(then_seek.unwrap_or(0));
                        self.seek(then_seek.unwrap_or(0));
                        return Ok(());
                    }
                    let q = &queue[qi];
                    let qframes = col.replays[q].frames().unwrap_or(0);
                    if frame < qframes {
                        if frame == 0 {
                            s.restore_inputs(col, q, 0)?;
                        }
                        s.step(col, q, frame, false)?;
                        frame += 1;
                        done += 1;
                    }
                    if frame >= qframes {
                        qi += 1;
                        frame = 0;
                    }
                    if t0.elapsed() > JOB_SLICE {
                        self.job = Some(Job::Enter { id, queue, qi, frame, start_frame, done, total, then_seek });
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Progress of the current job, for the status bar.
    fn job_progress(&self) -> Option<(f32, String)> {
        match self.job.as_ref()? {
            Job::Seek { from, to } => {
                let ran = self.pb.as_ref().map_or(*from, |p| p.ran);
                let f = (ran.saturating_sub(*from)) as f32 / (to - from).max(1) as f32;
                Some((f, format!("Seeking to frame {}", to.saturating_sub(1))))
            }
            Job::Enter { id, done, total, .. } => {
                let title = col_title(self.col.as_ref(), id);
                Some((*done as f32 / (*total).max(1) as f32, format!("Playing up to \"{title}\"")))
            }
        }
    }

    /// Runs one frame of the selected replay.
    fn run_one(&mut self, capture: bool) -> Result<()> {
        let (Some(col), Some(s), Some(pb)) = (&self.col, &mut self.session, &mut self.pb) else { return Ok(()) };
        if pb.ran >= pb.frames {
            return Ok(());
        }
        let k = pb.ran;
        if k % SNAP_EVERY == 0 && !pb.snapshots.contains_key(&k) {
            pb.snapshots.insert(k, s.snapshot()?);
        }
        if let Some(want) = pb.checkpoints.get(&k) {
            let got = s.state_hash(col)?;
            if &got != want && pb.checkpoint_miss.is_none_or(|m| k < m) {
                pb.checkpoint_miss = Some(k);
            }
        }
        let frame = s.step(col, &pb.id, k, capture || k + 1 == pb.frames)?;
        pb.ran += 1;
        if pb.ran == pb.frames {
            pb.end_hash = Some(s.state_hash(col)?);
        }
        if let Some(f) = frame {
            let img = egui::ColorImage::from_rgb([f.width as usize, f.height as usize], &f.rgb);
            match &mut self.texture {
                Some(t) => t.set(img, egui::TextureOptions::NEAREST),
                None => self.texture = Some(egui_ctx().load_texture("preview", img, egui::TextureOptions::NEAREST)),
            }
        }
        Ok(())
    }

    /// Shows frame `target` (0-based) of the selected replay.
    fn seek(&mut self, target: u64) {
        if let Some(Job::Enter { then_seek, .. }) = &mut self.job {
            *then_seek = Some(target);
            return;
        }
        if matches!(self.job, Some(Job::Seek { .. })) {
            self.job = None;
        }
        if let Err(e) = self.try_seek(target) {
            self.playing = false;
            self.error(e);
        }
    }

    fn try_seek(&mut self, target: u64) -> Result<()> {
        let Some(pb) = &self.pb else { return Ok(()) };
        if pb.frames == 0 {
            return Ok(());
        }
        let want_ran = (target + 1).min(pb.frames);
        if want_ran != pb.ran || self.texture.is_none() {
            // Restart from the nearest snapshot that still leaves one frame to draw,
            // unless running on from where we are is quicker.
            let (&k, _) = pb.snapshots.range(..want_ran).next_back().ok_or_else(|| anyhow!("no snapshot"))?;
            if want_ran <= pb.ran || k > pb.ran {
                let id = pb.id.clone();
                let state = pb.snapshots[&k].clone();
                let (Some(col), Some(s)) = (&self.col, &mut self.session) else { return Ok(()) };
                s.load_state(&state)?;
                s.restore_inputs(col, &id, k)?;
                let pb = self.pb.as_mut().unwrap();
                pb.ran = k;
                pb.end_hash = None;
            }
            let ran = self.pb.as_ref().unwrap().ran;
            if want_ran - ran > 30 {
                // Far ahead: run it in slices so the window stays responsive.
                self.job = Some(Job::Seek { from: ran, to: want_ran });
                self.work();
                return Ok(());
            }
            while self.pb.as_ref().unwrap().ran < want_ran {
                let last = self.pb.as_ref().unwrap().ran + 1 == want_ran;
                self.run_one(last)?;
            }
        }
        Ok(())
    }

    fn step_by(&mut self, delta: i64) {
        self.playing = false;
        if let Some(cur) = self.pb.as_ref().and_then(|p| p.shown()) {
            let t = (cur as i64 + delta).max(0) as u64;
            self.seek(t);
            self.roll.follow_to(t);
        }
    }

    fn toggle_play(&mut self) {
        if let Some(pb) = &self.pb {
            if !self.playing && pb.ran >= pb.frames {
                self.seek(0);
            }
            self.playing = !self.playing;
            self.clock = 0.0;
        }
    }

    /// Double-click in the tree: select the node if needed and play it.
    /// While the node is still loading, playback starts once it is ready.
    fn play_node(&mut self, id: &str) {
        if self.selected.as_deref() != Some(id) {
            self.select(id);
        }
        if self.pb.is_none() {
            self.playing = self.job.is_some();
            self.clock = 0.0;
        } else if !self.playing {
            self.toggle_play();
        }
    }

    fn at_end(&self) -> bool {
        self.pb.as_ref().is_some_and(|p| p.ran >= p.frames)
    }

    fn tick(&mut self, dt: f64) {
        if !self.playing || self.job.is_some() {
            self.clock = 0.0;
            return;
        }
        let res = if self.speed == 0 {
            // As fast as the core goes, drawing once per screen refresh.
            let t0 = std::time::Instant::now();
            let mut r = Ok(());
            while !self.at_end() && r.is_ok() {
                let last = t0.elapsed() >= std::time::Duration::from_millis(14);
                r = self.run_one(last);
                if last {
                    break;
                }
            }
            r
        } else {
            let rate = self.fps * self.speed as f64;
            self.clock += dt;
            let mut n = (self.clock * rate).floor() as u64;
            self.clock -= n as f64 / rate;
            // Never fall far behind; slow down instead.
            let cap = 4 * self.speed as u64;
            if n > cap {
                n = cap;
                self.clock = 0.0;
            }
            let mut r = Ok(());
            for i in 0..n {
                r = self.run_one(i + 1 == n);
                if r.is_err() {
                    break;
                }
            }
            r
        };
        if let Err(e) = res {
            self.playing = false;
            self.error(e);
            return;
        }
        if self.at_end() {
            if !self.route.is_empty() {
                let next = self.route.remove(0);
                if let Err(e) = self.continue_into(&next) {
                    self.playing = false;
                    self.error(e);
                }
                return;
            }
            if self.route_target.is_some() && self.route_target == self.selected {
                self.route_target = None;
                self.playing = false;
                return;
            }
            match self.next_node() {
                Some(child) if self.auto_continue => {
                    if let Err(e) = self.continue_into(&child) {
                        self.playing = false;
                        self.error(e);
                    }
                }
                _ => self.playing = false,
            }
        }
    }

    /// Plays through to the end of `target`: on from the playhead when the current node is
    /// on the way there, otherwise from the start of the first node below where the two
    /// paths meet (or from the trunk).
    fn play_to(&mut self, target: &str) {
        let Some(col) = &self.col else { return };
        let Ok(path) = col.chain(target) else { return };
        let path: Vec<String> = path.into_iter().map(String::from).collect();
        let cur = self.selected.clone();
        let on_path = cur.as_ref().and_then(|c| path.iter().position(|p| p == c));
        let (start, from_playhead) = match on_path {
            Some(i) => (i, true),
            None => {
                let cur_chain: Vec<String> = cur.as_deref().and_then(|c| col.chain(c).ok()).unwrap_or_default().into_iter().map(String::from).collect();
                let common = path.iter().zip(cur_chain.iter()).take_while(|(a, b)| a == b).count();
                // `common` nodes are shared; start just below them, or at the target if it is one of them.
                (common.min(path.len() - 1), false)
            }
        };
        if !from_playhead {
            self.select(&path[start]);
        } else if self.at_end() && start == path.len() - 1 {
            self.seek(0);
        }
        self.route = path[start + 1..].to_vec();
        self.route_target = Some(target.to_string());
        self.playing = self.pb.is_some();
        self.clock = 0.0;
        let title = col_title(self.col.as_ref(), target);
        self.info(format!("Playing to the end of \"{title}\" ({} node(s) on the way)", self.route.len() + 1));
    }

    /// The only child of the current node, if neither has unsaved changes.
    fn next_node(&self) -> Option<String> {
        let (col, pb) = (self.col.as_ref()?, self.pb.as_ref()?);
        let kids = col.children(Some(&pb.id));
        if kids.len() != 1 || self.edited.contains(&pb.id) || self.edited.contains(kids[0]) {
            return None;
        }
        Some(kids[0].to_string())
    }

    /// Moves from the end of the current node straight into `child`, keeping the core's state.
    fn continue_into(&mut self, child: &str) -> Result<()> {
        let (Some(col), Some(s), Some(pb)) = (&self.col, &mut self.session, &self.pb) else { return Ok(()) };
        let start_frame = pb.start_frame + pb.frames;
        let start_hash = pb.end_hash.clone();
        s.restore_inputs(col, child, 0)?;
        let snap = s.snapshot()?;
        let r = &col.replays[child];
        let frames = r.frames().unwrap_or(0);
        let checkpoints = r.checkpoints().unwrap_or_default().into_iter().collect();
        self.pb = Some(Playback { id: child.to_string(), start_frame, frames, ran: 0, snapshots: BTreeMap::from([(0, snap)]), checkpoints, checkpoint_miss: None, end_hash: None, start_hash });
        self.selected = Some(child.to_string());
        self.roll.follow_to(0);
        Ok(())
    }

    // ---- editing ----

    fn apply_edit(&mut self, label: &str, edits: Vec<(InputKey, Vec<(u64, u64)>)>, from: u64) {
        let Some(id) = self.selected.clone() else { return };
        self.remember(label, &[&id]);
        let Some(col) = &mut self.col else { return };
        let r = col.replays.get_mut(&id).unwrap();
        for (key, spans) in edits {
            if let Err(e) = r.set_binary_spans(key, &spans) {
                self.error(e);
                return;
            }
        }
        self.invalidate_from(&id, from);
    }

    /// After a change at frame `from` of `id`: drops the end hash and later checkpoints
    /// (so the node and everything below turn unverified or dirty) and redraws.
    fn invalidate_from(&mut self, id: &str, from: u64) {
        let Some(col) = &mut self.col else { return };
        let id = id.to_string();
        let r = col.replays.get_mut(&id).unwrap();
        // The end state and later checkpoints are no longer known; children turn dirty.
        r.remove_all("end-hash");
        let keep: Vec<(u64, String)> = r.checkpoints().unwrap_or_default().into_iter().filter(|(f, _)| *f <= from).collect();
        r.remove_all("checkpoint");
        for (f, h) in &keep {
            r.add("checkpoint", &format!("{f} {h}"));
        }
        self.edited.insert(id.clone());
        self.roll.invalidate();
        if let Some(pb) = &mut self.pb {
            if pb.id == id {
                pb.snapshots.retain(|k, _| *k <= from);
                pb.checkpoints = keep.into_iter().collect();
                pb.checkpoint_miss = pb.checkpoint_miss.filter(|m| *m <= from);
                pb.end_hash = None;
                if let Some(cur) = pb.shown() {
                    // Redraw the current frame from before the edit.
                    pb.ran = 0;
                    self.seek(cur);
                }
            }
        }
    }

    /// Records the state hash playback reached at the end as the replay's end-hash.
    fn accept_end_hash(&mut self) {
        if let Some(id) = self.pb.as_ref().filter(|p| p.end_hash.is_some()).map(|p| p.id.clone()) {
            self.remember("Accept end hash", &[&id]);
        }
        let (Some(col), Some(pb)) = (&mut self.col, &self.pb) else { return };
        let Some(h) = pb.end_hash.clone() else { return };
        let r = col.replays.get_mut(&pb.id).unwrap();
        r.set("end-hash", &h);
        r.set("start-frame", &pb.start_frame.to_string());
        if let Some(sh) = &pb.start_hash {
            r.set("start-hash", sh);
        }
        self.edited.insert(pb.id.clone());
        self.info("Accepted the new end hash. Save to keep it; child replays will check against it.");
    }

    // ---- undo, redo and clipboard ----

    /// Records the nodes `ids` as they are now, before a change, as one undo step.
    fn remember(&mut self, label: &str, ids: &[&str]) {
        let Some(col) = &self.col else { return };
        let nodes = ids.iter().map(|id| (id.to_string(), col.replays.get(*id).cloned())).collect();
        let frame = self.pb.as_ref().and_then(|p| p.shown());
        self.undo.push(UndoStep { label: label.to_string(), nodes, selected: self.selected.clone(), frame });
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn undo(&mut self) {
        if let Some(step) = self.undo.pop() {
            let label = step.label.clone();
            let back = self.restore(step);
            self.redo.push(back);
            self.info(format!("Undid: {label}"));
        }
    }

    fn redo(&mut self) {
        if let Some(step) = self.redo.pop() {
            let label = step.label.clone();
            let back = self.restore(step);
            self.undo.push(back);
            self.info(format!("Redid: {label}"));
        }
    }

    /// Puts the nodes of `step` back and returns the step that reverses it.
    fn restore(&mut self, step: UndoStep) -> UndoStep {
        self.renaming = None;
        self.playing = false;
        let here = self.pb.as_ref().and_then(|p| p.shown());
        let cur = self.selected.clone();
        let Some(col) = &mut self.col else { return step };
        let chain: Vec<String> = cur.as_deref().and_then(|c| col.chain(c).ok()).unwrap_or_default().into_iter().map(String::from).collect();
        // A change to the current node alone, keeping its length and parent, only needs the
        // frames from the first difference on to be run again. Anything else reloads the node.
        let mut first_diff: Option<u64> = None;
        let mut reload = false;
        let mut back = vec![];
        for (id, before) in step.nodes {
            let now = col.replays.get(&id).cloned();
            if cur.as_deref() == Some(id.as_str()) {
                match (&now, &before) {
                    (Some(a), Some(b)) if a.frames().ok() == b.frames().ok() && a.parent() == b.parent() => {
                        if let Some(f) = first_change(a, b) {
                            first_diff = Some(first_diff.map_or(f, |d| d.min(f)));
                        }
                    }
                    _ => reload = true,
                }
            } else if chain.contains(&id) {
                reload = true;
            }
            match before {
                Some(r) => {
                    col.replays.insert(id.clone(), r);
                }
                None => {
                    col.replays.remove(&id);
                }
            }
            self.edited.insert(id.clone());
            back.push((id, now));
        }
        let reverse = UndoStep { label: step.label, nodes: back, selected: cur.clone(), frame: here };
        self.roll.invalidate();
        let target = step.selected.filter(|s| col.replays.contains_key(s));
        if self.hoist.as_ref().is_some_and(|h| !col.replays.contains_key(h)) {
            self.hoist = None;
        }
        if target != cur || reload {
            match &target {
                Some(t) => {
                    self.select(t);
                    let f = step.frame.unwrap_or(0);
                    self.seek(f);
                    self.roll.follow_to(f);
                }
                None => {
                    self.selected = None;
                    self.pb = None;
                    self.texture = None;
                }
            }
        } else if let (Some(from), Some(id)) = (first_diff, &cur) {
            // Same node: drop what playback knew past the first change and redraw here.
            let checkpoints: BTreeMap<u64, String> = col.replays[id].checkpoints().unwrap_or_default().into_iter().collect();
            if let Some(pb) = &mut self.pb {
                pb.snapshots.retain(|k, _| *k <= from);
                pb.checkpoints = checkpoints;
                pb.checkpoint_miss = None;
                pb.end_hash = None;
                pb.ran = 0;
                let f = here.unwrap_or(0);
                self.seek(f);
            }
        }
        self.refresh_frame_dialog();
        reverse
    }

    fn copy(&mut self) -> bool {
        let notes = self.roll.selected_notes();
        let Some(base) = notes.iter().map(|(_, sp)| sp.0).min() else { return false };
        let n = notes.len();
        // Also put the notes on the system clipboard as replay lines (frames relative to the
        // first note). Some platforms only report Ctrl/Cmd+V when the clipboard holds text.
        let mut lines: Vec<(u64, String)> = vec![];
        for (k, sp) in &notes {
            lines.push((sp.0 - base, emucov::format::input_text(*k, 1)));
            lines.push((sp.1 - base, emucov::format::input_text(*k, 0)));
        }
        lines.sort();
        let text: Vec<String> = lines.into_iter().map(|(f, t)| format!("{f} {t}")).collect();
        self.ctx_copy = Some(text.join("\n") + "\n");
        self.clip = Some(Clip { notes: notes.into_iter().map(|(k, sp)| (k, sp.0 - base, sp.1 - sp.0)).collect() });
        self.info(format!("Copied {n} note{}", if n == 1 { "" } else { "s" }));
        true
    }

    fn cut(&mut self) {
        let notes = self.roll.selected_notes();
        if notes.is_empty() || !self.copy() {
            return;
        }
        let (Some(col), Some(id)) = (&self.col, &self.selected) else { return };
        let r = &col.replays[id];
        let keys: BTreeSet<InputKey> = notes.iter().map(|(k, _)| *k).collect();
        let edits = keys.into_iter().map(|k| (k, r.binary_spans(k).into_iter().filter(|sp| !notes.contains(&(k, *sp))).collect())).collect();
        let from = notes.iter().map(|(_, sp)| sp.0).min().unwrap_or(0);
        self.roll.set_selection([]);
        self.apply_edit("Cut", edits, from);
    }

    /// Pastes the copied notes so the leftmost one starts at the play cursor. The pasted
    /// notes become the selection; notes past the node's end are cut off.
    fn paste(&mut self) {
        let (Some(clip), Some(col), Some(id), Some(pb)) = (&self.clip, &self.col, &self.selected, &self.pb) else { return };
        let at = pb.shown().unwrap_or(0);
        let r = &col.replays[id];
        let frames = r.frames().unwrap_or(0);
        let keys: BTreeSet<InputKey> = clip.notes.iter().map(|(k, _, _)| *k).collect();
        let mut edits = vec![];
        let mut pasted = vec![];
        for key in keys {
            let mut spans = r.binary_spans(key);
            let mut starts = vec![];
            for (_, rel, len) in clip.notes.iter().filter(|(k, _, _)| *k == key) {
                let a = at + rel;
                if a < frames {
                    spans.push((a, (a + len).min(frames)));
                    starts.push(a);
                }
            }
            if starts.is_empty() {
                continue;
            }
            let spans = roll::merge(spans);
            for a in starts {
                if let Some(sp) = spans.iter().find(|sp| sp.0 <= a && a < sp.1) {
                    pasted.push((key, sp.0));
                }
            }
            edits.push((key, spans));
        }
        if edits.is_empty() {
            self.info("Nothing to paste here: the play cursor is past the copied notes' room");
            return;
        }
        self.roll.set_selection(pasted);
        self.roll.mode = roll::Mode::Select;
        self.apply_edit("Paste", edits, at);
    }

    // ---- nodes ----

    /// Deletes `id` and everything below it (`ids`, from `subtree`), as one undo step.
    fn delete_nodes(&mut self, id: &str, ids: Vec<String>) {
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        self.remember("Delete", &refs);
        let Some(col) = &mut self.col else { return };
        let parent = col.replays.get(id).and_then(|r| r.parent().map(String::from));
        for i in &ids {
            col.replays.remove(i);
            self.edited.insert(i.clone());
            self.tree_closed.remove(i);
        }
        if self.hoist.as_ref().is_some_and(|h| ids.contains(h)) {
            self.hoist = None;
        }
        self.roll.invalidate();
        let n = ids.len();
        // If the shown node went, show its parent instead.
        if self.selected.as_ref().is_some_and(|s| ids.contains(s)) {
            self.pb = None;
            self.texture = None;
            self.selected = None;
            if let Some(p) = parent {
                self.select(&p);
            }
        }
        self.info(format!("Deleted {n} node{}. Save to remove the files, or undo.", if n == 1 { "" } else { "s" }));
    }

    fn new_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    /// Adds an empty child node that starts where `parent` ends, and selects it.
    fn new_child(&mut self, parent: &str) {
        let id = Self::new_id();
        self.remember("New node", &[&id]);
        let Some(col) = &mut self.col else { return };
        let r = emucov::format::Replay::new(&id, Some(parent), "New node", 600);
        col.replays.insert(id.clone(), r);
        self.edited.insert(id.clone());
        self.tree_closed.remove(parent);
        self.info("Added a 600-frame node. Change its title and length with Node > Properties.");
        self.select(&id);
        self.open_node_dialog(&id);
    }

    /// Splits the selected node at the playhead: frames before it stay in this node,
    /// the rest move to a new child that inherits this node's children. With `branch`,
    /// also adds an empty sibling of that child, so a second path starts at the playhead.
    fn split_here(&mut self, branch: bool) {
        if let Err(e) = self.try_split(branch) {
            self.error(e);
        }
    }

    fn try_split(&mut self, branch: bool) -> Result<()> {
        let pb = self.pb.as_ref().ok_or_else(|| anyhow!("select a node first"))?;
        let id = pb.id.clone();
        // The playhead shows frame f; the split goes right after it, so frame f stays upstream.
        let f = pb.shown().unwrap_or(0) + 1;
        if f >= pb.frames {
            if branch {
                self.new_child(&id);
                return Ok(());
            }
            return Err(anyhow!("the playhead is on the last frame: use New child at end instead"));
        }
        self.playing = false;
        // State hash at the start of frame f: run exactly f frames.
        self.seek(f - 1);
        let (Some(col), Some(s)) = (&mut self.col, &mut self.session) else { return Ok(()) };
        let h = s.state_hash(col)?;
        let up_id = Self::new_id();
        let b = Self::new_id();
        {
            let mut ids = vec![id.as_str(), up_id.as_str()];
            if branch {
                ids.push(b.as_str());
            }
            self.remember(if branch { "Branch" } else { "Split" }, &ids);
        }
        let (Some(col), Some(_)) = (&mut self.col, &mut self.session) else { return Ok(()) };
        let (mut up, mut down) = col.replays[&id].split_at(f, &up_id)?;
        if let Some((_, want)) = col.replays[&id].checkpoints()?.into_iter().find(|(cf, _)| *cf == f) {
            if want != h {
                self.message = Some((format!("Note: the recorded checkpoint at frame {f} does not match this playback"), true));
            }
        }
        // The cut gets a fresh state hash. The second half keeps the node's id, children and
        // old end hash, so it and everything below are assumed clean.
        up.set("end-hash", &h);
        down.set("start-hash", &h);
        col.replays.insert(up_id.clone(), up);
        col.replays.insert(id.clone(), down);
        self.edited.insert(id.clone());
        self.edited.insert(up_id.clone());
        self.roll.invalidate();
        if branch {
            let r = emucov::format::Replay::new(&b, Some(&up_id), "New branch", 600);
            col.replays.insert(b.clone(), r);
            self.edited.insert(b.clone());
            self.info(format!("Split at frame {f}: the first {f} frames are a new parent node, and a new branch starts beside the rest."));
            self.select(&b);
            self.open_node_dialog(&b);
        } else {
            self.info(format!("Split at frame {f}: the first {f} frames are now a new parent node."));
            self.select(&up_id);
            self.seek(f - 1);
        }
        Ok(())
    }

    fn open_node_dialog(&mut self, id: &str) {
        let Some(col) = &self.col else { return };
        let r = &col.replays[id];
        self.node_dialog = Some(NodeDialog { id: id.to_string(), title: r.title().to_string(), frames: r.frames().unwrap_or(0) });
    }

    fn apply_node_dialog(&mut self, d: &NodeDialog) -> Result<()> {
        let Some(col) = &mut self.col else { return Ok(()) };
        let r = col.replays.get_mut(&d.id).ok_or_else(|| anyhow!("node is gone"))?;
        let old_frames = r.frames()?;
        if let Some(last) = r.lines.last() {
            if d.frames <= last.frame {
                return Err(anyhow!("frame {} has lines; delete them first or keep at least {} frames", last.frame, last.frame + 1));
            }
        }
        if d.frames == 0 {
            return Err(anyhow!("a node needs at least one frame"));
        }
        if d.title.trim() == r.title() && d.frames == old_frames {
            return Ok(());
        }
        self.remember("Node properties", &[&d.id]);
        let Some(col) = &mut self.col else { return Ok(()) };
        let r = col.replays.get_mut(&d.id).ok_or_else(|| anyhow!("node is gone"))?;
        r.set("title", d.title.trim());
        self.edited.insert(d.id.clone());
        if d.frames != old_frames {
            r.set("frames", &d.frames.to_string());
            self.invalidate_from(&d.id.clone(), d.frames.min(old_frames));
            if self.selected.as_deref() == Some(d.id.as_str()) {
                let cur = self.pb.as_ref().and_then(|p| p.shown()).unwrap_or(0);
                self.select(&d.id.clone());
                self.seek(cur.min(d.frames - 1));
            }
        }
        self.roll.invalidate();
        Ok(())
    }

    fn open_frame_dialog(&mut self, frame: u64, new_text: &str) {
        self.frame_dialog = Some(FrameDialog { frame, rows: vec![], new_text: new_text.to_string(), focus: true });
        self.refresh_frame_dialog();
    }

    fn refresh_frame_dialog(&mut self) {
        let (Some(col), Some(id), Some(d)) = (&self.col, &self.selected, &mut self.frame_dialog) else { return };
        let r = &col.replays[id];
        d.rows = r.lines.iter().filter(|l| l.frame == d.frame && !matches!(l.command, Command::Input { .. })).filter_map(|l| r.line_command_text(l.line_no).map(|t| (l.line_no, t))).collect();
    }

    /// Changes a non-input line on the selected node: replace (Some) or delete (None) an
    /// existing line, or add a new one when `line_no` is None.
    fn edit_line(&mut self, frame: u64, line_no: Option<usize>, text: Option<&str>) {
        let (Some(col), Some(id)) = (&mut self.col, self.selected.clone()) else { return };
        let r = col.replays.get(&id).unwrap();
        // Events and screenshots don't touch the game; everything else may.
        let harmless = |t: &str| {
            let t = t.trim_start().to_ascii_uppercase();
            t.starts_with("[EVENT") || t.starts_with("[SCREENSHOT")
        };
        let old_text = line_no.and_then(|n| r.line_command_text(n));
        if let (Some(o), Some(t)) = (&old_text, text) {
            if o.trim() == t.trim() {
                return;
            }
        }
        if line_no.is_none() && text.is_none() {
            return;
        }
        self.remember(if text.is_none() { "Delete line" } else if line_no.is_none() { "Add line" } else { "Edit line" }, &[&id]);
        let Some(col) = &mut self.col else { return };
        let r = col.replays.get_mut(&id).unwrap();
        let res = match (line_no, text) {
            (Some(n), t) => r.replace_line(n, t),
            (None, Some(t)) => r.insert_line(frame, t),
            (None, None) => Ok(()),
        };
        if let Err(e) = res {
            self.error(e);
            return;
        }
        self.edited.insert(id.clone());
        self.roll.invalidate();
        let changes_state = !(old_text.as_deref().is_none_or(harmless) && text.is_none_or(harmless));
        if changes_state {
            self.invalidate_from(&id, frame);
        }
        self.refresh_frame_dialog();
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        // One confirmation for Node > Delete and the tree's right-click Delete.
        let pending = self.delete_dialog.clone().and_then(|id| self.col.as_ref().filter(|c| c.replays.contains_key(&id)).map(|c| (id, c)));
        if pending.is_none() {
            self.delete_dialog = None;
        }
        if let Some((id, col)) = pending {
            let r = &col.replays[&id];
            let ids = subtree(col, &id);
            let frames: u64 = ids.iter().map(|i| col.replays[i].frames().unwrap_or(0)).sum();
            let title = r.title().to_string();
            let mut open = true;
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Delete node").open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
                ui.label(RichText::new(format!("Delete \"{title}\"?")).strong());
                let below = ids.len() - 1;
                if below > 0 {
                    ui.label(RichText::new(format!("This also deletes the {below} node{} below it.", if below == 1 { "" } else { "s" })).color(Color32::from_rgb(240, 160, 60)));
                }
                ui.label(format!("{} node{}, {frames} frames in all. Undo brings them back; the files are removed when you save.", ids.len(), if ids.len() == 1 { "" } else { "s" }));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new(if ids.len() == 1 { "Delete".to_string() } else { format!("Delete {} nodes", ids.len()) }).color(Color32::from_rgb(240, 110, 100))).clicked() {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });
            if confirm {
                self.delete_dialog = None;
                self.delete_nodes(&id, ids);
            } else if cancel || !open {
                self.delete_dialog = None;
            }
        }
        if let Some(mut d) = self.node_dialog.take() {
            let mut open = true;
            let mut apply = false;
            egui::Window::new("Node properties").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                egui::Grid::new("node_props").num_columns(2).show(ui, |ui| {
                    ui.label("Title");
                    ui.add(egui::TextEdit::singleline(&mut d.title).desired_width(320.0));
                    ui.end_row();
                    ui.label("Frames");
                    ui.add(egui::DragValue::new(&mut d.frames).range(1..=10_000_000).speed(10.0));
                    ui.end_row();
                    ui.label("");
                    ui.label(RichText::new(format!("{:.1} s at {:.2} fps", d.frames as f64 / self.fps, self.fps)).weak());
                    ui.end_row();
                });
                ui.label(RichText::new(format!("id {}", d.id)).weak().small());
                ui.horizontal(|ui| {
                    apply = ui.button("Apply").clicked();
                });
            });
            if apply {
                match self.apply_node_dialog(&d) {
                    Ok(()) => open = false,
                    Err(e) => self.error(e),
                }
            }
            if open {
                self.node_dialog = Some(d);
            }
        }
        if let Some(mut d) = self.frame_dialog.take() {
            let mut open = true;
            let mut change: Option<(Option<usize>, Option<String>)> = None;
            let title = format!("Frame {}", d.frame);
            egui::Window::new(title).id(egui::Id::unique("frame_dialog")).open(&mut open).collapsible(false).default_width(460.0).show(ctx, |ui| {
                ui.label(RichText::new("Events, RAM writes and other lines on this frame. Edit the text after the frame number.").weak());
                ui.add_space(4.0);
                if d.rows.is_empty() {
                    ui.label(RichText::new("No lines on this frame yet.").italics());
                }
                for (n, text) in &mut d.rows {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(text).desired_width(330.0).font(egui::TextStyle::Monospace));
                        if ui.button("Apply").clicked() {
                            change = Some((Some(*n), Some(text.clone())));
                        }
                        if ui.button("Delete").clicked() {
                            change = Some((Some(*n), None));
                        }
                    });
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("New:");
                    if ui.small_button("event").clicked() {
                        d.new_text = "[EVENT] ".into();
                    }
                    if ui.small_button("RAM write").clicked() {
                        d.new_text = "[WRITE] RAM 0x0000 00".into();
                    }
                });
                ui.horizontal(|ui| {
                    let te = ui.add(egui::TextEdit::singleline(&mut d.new_text).desired_width(330.0).font(egui::TextStyle::Monospace).hint_text("[EVENT] Boss appears"));
                    if d.focus {
                        te.request_focus();
                        d.focus = false;
                    }
                    let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui.button("Add").clicked() || enter) && !d.new_text.trim().is_empty() {
                        change = Some((None, Some(d.new_text.clone())));
                    }
                });
                ui.label(RichText::new("A write takes any number of bytes: [WRITE] RAM 0x0300 01 02 03 …").weak().small());
            });
            let frame = d.frame;
            let added = matches!(change, Some((None, Some(_))));
            self.frame_dialog = open.then_some(d);
            if let Some((n, t)) = change {
                self.edit_line(frame, n, t.as_deref());
                if added && !self.message.as_ref().is_some_and(|m| m.1) {
                    if let Some(d) = &mut self.frame_dialog {
                        d.new_text = "[EVENT] ".into();
                        d.focus = true;
                    }
                }
            }
        }
    }

    // ---- UI ----

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open collection…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().set_title("Open a replay collection folder").pick_folder() {
                        self.open_collection(&dir);
                    }
                }
                let n = self.edited.len();
                if ui.add_enabled(n > 0, egui::Button::new(format!("Save ({n} changed)")).shortcut_text("Ctrl+S")).clicked() {
                    self.save();
                }
                if ui.add_enabled(self.col.is_some(), egui::Button::new("Reload from disk")).on_hover_text("Discards unsaved edits").clicked() {
                    self.reload();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                use egui::{Key, KeyboardShortcut, Modifiers};
                let ctx = ui.ctx().clone();
                let sc = |m: Modifiers, k: Key| ctx.format_shortcut(&KeyboardShortcut::new(m, k));
                let undo_label = self.undo.last().map_or("Undo".to_string(), |s| format!("Undo {}", s.label.to_lowercase()));
                if ui.add_enabled(!self.undo.is_empty(), egui::Button::new(undo_label).shortcut_text(sc(Modifiers::COMMAND, Key::Z))).clicked() {
                    self.undo();
                }
                let redo_label = self.redo.last().map_or("Redo".to_string(), |s| format!("Redo {}", s.label.to_lowercase()));
                if ui.add_enabled(!self.redo.is_empty(), egui::Button::new(redo_label).shortcut_text(sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z))).clicked() {
                    self.redo();
                }
                ui.separator();
                let has_sel = !self.roll.selected_notes().is_empty();
                if ui.add_enabled(has_sel, egui::Button::new("Cut").shortcut_text(sc(Modifiers::COMMAND, Key::X))).clicked() {
                    self.cut();
                }
                if ui.add_enabled(has_sel, egui::Button::new("Copy").shortcut_text(sc(Modifiers::COMMAND, Key::C))).clicked() {
                    self.copy();
                }
                if ui.add_enabled(self.clip.is_some() && self.pb.is_some(), egui::Button::new("Paste at play cursor").shortcut_text(sc(Modifiers::COMMAND, Key::V))).clicked() {
                    self.paste();
                }
            });
            ui.menu_button("Emulation", |ui| {
                let has = self.pb.is_some();
                if ui.add_enabled(has, egui::Button::new(if self.playing { "Pause" } else { "Play" }).shortcut_text("Space")).clicked() {
                    self.toggle_play();
                }
                if ui.add_enabled(has, egui::Button::new("Step forward").shortcut_text("→")).clicked() {
                    self.step_by(1);
                }
                if ui.add_enabled(has, egui::Button::new("Step back").shortcut_text("←")).clicked() {
                    self.step_by(-1);
                }
                if ui.add_enabled(has, egui::Button::new("Go to start").shortcut_text("Home")).clicked() {
                    self.playing = false;
                    self.seek(0);
                    self.roll.follow_to(0);
                }
                if ui.add_enabled(has, egui::Button::new("Go to end").shortcut_text("End")).clicked() {
                    self.playing = false;
                    let end = self.pb.as_ref().map_or(0, |p| p.frames.saturating_sub(1));
                    self.seek(end);
                    self.roll.follow_to(end);
                }
                ui.separator();
                let can_accept = self.pb.as_ref().is_some_and(|p| p.end_hash.is_some()) && self.end_matches() != Some(true);
                if ui.add_enabled(can_accept, egui::Button::new("Accept end hash")).on_hover_text("Play to the last frame first").clicked() {
                    self.accept_end_hash();
                }
                ui.separator();
                let has_col = self.col.is_some();
                if ui.add_enabled(has_col, egui::Button::new("Set ROM…")).clicked() {
                    if let Some(f) = rfd::FileDialog::new().set_title("Choose the ROM").pick_file() {
                        self.set_local(Some(f), None);
                    }
                }
                if ui.add_enabled(has_col, egui::Button::new("Set core…")).clicked() {
                    if let Some(f) = rfd::FileDialog::new().set_title("Choose the libretro core").add_filter("libretro core", &["so", "dylib", "dll"]).pick_file() {
                        self.set_local(None, Some(f));
                    }
                }
            });
            ui.menu_button("Node", |ui| {
                let sel = self.selected.clone();
                let has = sel.is_some() && self.pb.is_some();
                if ui.add_enabled(has, egui::Button::new("New child at end")).on_hover_text("A new empty node that starts where this one ends").clicked() {
                    self.new_child(sel.as_deref().unwrap());
                }
                if ui.add_enabled(has, egui::Button::new("Branch at playhead")).on_hover_text("Split this node after the playhead and start a second path there").clicked() {
                    self.split_here(true);
                }
                if ui.add_enabled(has, egui::Button::new("Split at playhead")).on_hover_text("Cut this node in two after the playhead; its children move to the second half").clicked() {
                    self.split_here(false);
                }
                ui.separator();
                if ui.add_enabled(has, egui::Button::new("Add event at playhead…").shortcut_text("E")).clicked() {
                    let f = self.pb.as_ref().and_then(|p| p.shown()).unwrap_or(0);
                    self.open_frame_dialog(f, "[EVENT] ");
                }
                if ui.add_enabled(sel.is_some(), egui::Button::new("Properties…")).clicked() {
                    self.open_node_dialog(sel.as_deref().unwrap());
                }
                ui.separator();
                if ui.add_enabled(sel.is_some(), egui::Button::new("Delete…")).on_hover_text("Delete this node and everything below it").clicked() {
                    self.delete_dialog = sel.clone();
                }
            });
            ui.menu_button("Input", |ui| {
                ui.add_enabled(false, egui::Button::new("Keyboard mapping…")).on_disabled_hover_text("Coming later: play the game from the keyboard and record into the roll");
            });
            ui.menu_button("View", |ui| {
                if ui.button("Zoom in").clicked() {
                    self.roll.zoom(1.5);
                }
                if ui.button("Zoom out").clicked() {
                    self.roll.zoom(1.0 / 1.5);
                }
                ui.checkbox(&mut self.roll.follow, "Follow playhead");
                ui.checkbox(&mut self.roll.all_buttons, "Show every joypad button");
                ui.add_enabled(false, egui::Checkbox::new(&mut false, "Second core side by side")).on_disabled_hover_text("Coming later");
            });
        });
    }

    fn end_matches(&self) -> Option<bool> {
        let (col, pb) = (self.col.as_ref()?, self.pb.as_ref()?);
        let got = pb.end_hash.as_ref()?;
        Some(col.replays[&pb.id].get("end-hash") == Some(got.as_str()))
    }

    fn tree(&mut self, ui: &mut egui::Ui) {
        let Some(col) = &self.col else {
            ui.add_space(8.0);
            ui.label("No collection open.");
            if ui.button("Open collection…").clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.open_collection(&dir);
                }
            }
            return;
        };
        ui.add_space(4.0);
        ui.heading(&col.config.name);
        let (mut clean, mut dirty, mut unv) = (0, 0, 0);
        for id in col.replays.keys() {
            match col.tree_status(id) {
                Status::Clean => clean += 1,
                Status::Dirty => dirty += 1,
                Status::Unverified => unv += 1,
            }
        }
        ui.horizontal(|ui| {
            for (n, s) in [(clean, Status::Clean), (dirty, Status::Dirty), (unv, Status::Unverified)] {
                status_dot(ui, s);
                ui.label(RichText::new(format!("{n} {}", s.label())).color(status_color(s)).small());
            }
        });
        // Breadcrumbs while the tree is focused on one node.
        let mut new_hoist: Option<Option<String>> = None;
        let hoist: Option<&str> = self.hoist.as_deref().and_then(|h| col.replays.get_key_value(h)).map(|(k, _)| k.as_str());
        if self.hoist.is_some() && hoist.is_none() {
            new_hoist = Some(None);
        }
        if let Some(h) = hoist {
            let chain = col.chain(h).unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                if ui.small_button("All").on_hover_text("Show the whole tree").clicked() {
                    new_hoist = Some(None);
                }
                let n = chain.len();
                // Long paths show the first node, a menu for the middle, and the last three.
                let (head, hidden, tail) = if n > 5 { (&chain[..1], &chain[1..n - 3], &chain[n - 3..]) } else { (&chain[..0], &chain[..0], &chain[..]) };
                let short = |t: &str| if t.chars().count() > 18 { format!("{}…", t.chars().take(17).collect::<String>()) } else { t.to_string() };
                let crumb = |ui: &mut egui::Ui, id: &str, new_hoist: &mut Option<Option<String>>| {
                    ui.label(RichText::new("›").weak());
                    let t = col.replays[id].title();
                    if id == h {
                        ui.label(RichText::new(short(t)).strong()).on_hover_text(t);
                    } else if ui.link(short(t)).on_hover_text(t).clicked() {
                        *new_hoist = Some(Some(id.to_string()));
                    }
                };
                for id in head {
                    crumb(ui, id, &mut new_hoist);
                }
                if !hidden.is_empty() {
                    ui.label(RichText::new("›").weak());
                    ui.menu_button(format!("… {}", hidden.len()), |ui| {
                        for id in hidden {
                            if ui.button(col.replays[*id].title()).clicked() {
                                new_hoist = Some(Some(id.to_string()));
                            }
                        }
                    });
                }
                for id in tail {
                    crumb(ui, id, &mut new_hoist);
                }
            });
        }
        // Right to left, so the field takes exactly the width left over and never pushes the panel wider.
        let mut fold: Option<bool> = None;
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 22.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if fold_button(ui, false).on_hover_text("Expand all").clicked() {
                fold = Some(false);
            }
            if fold_button(ui, true).on_hover_text("Collapse all").clicked() {
                fold = Some(true);
            }
            ui.add_space(2.0);
            if !self.filter.is_empty() && ui.small_button("✖").on_hover_text("Clear the filter").clicked() {
                self.filter.clear();
            }
            let w = ui.available_width();
            ui.add(egui::TextEdit::singleline(&mut self.filter).id_salt("tree_filter").hint_text("Filter: title, id or tag").desired_width(w));
        });
        match fold {
            // Collapse every node that has children, below the focused node if there is one,
            // so the roots (or the focused node's children) stay visible.
            Some(true) => {
                for id in col.replays.keys() {
                    let inside = hoist.is_none_or(|h| h != id && col.chain(id).is_ok_and(|c| c.contains(&h)));
                    if inside && !col.children(Some(id)).is_empty() {
                        self.tree_closed.insert(id.clone());
                    }
                }
            }
            Some(false) => self.tree_closed.clear(),
            None => {}
        }
        // While filtering, show the matches and the path down to each of them, ignoring collapsed nodes.
        let needle = self.filter.trim().to_lowercase();
        let (matches, shown): (HashSet<&str>, HashSet<&str>) = if needle.is_empty() {
            Default::default()
        } else {
            let mut m = HashSet::new();
            let mut sh = HashSet::new();
            for (id, r) in &col.replays {
                let hit = r.title().to_lowercase().contains(&needle)
                    || id.to_lowercase().starts_with(&needle)
                    || r.get("tags").is_some_and(|t| t.split_whitespace().any(|t| t.to_lowercase().contains(&needle)));
                let chain = col.chain(id).unwrap_or_default();
                if hit && hoist.is_none_or(|h| chain.contains(&h)) {
                    m.insert(id.as_str());
                    for a in chain {
                        sh.insert(a);
                    }
                }
            }
            (m, sh)
        };
        let filtering = !needle.is_empty();
        if filtering {
            ui.label(RichText::new(match matches.len() {
                0 => "No matching nodes".to_string(),
                1 => "1 match".to_string(),
                n => format!("{n} matches"),
            }).weak().small());
        }
        ui.separator();
        let mut clicked: Option<String> = None;
        let mut double: Option<String> = None;
        let mut rename_done: Option<(String, Option<String>)> = None;
        let mut toggled: Option<String> = None;
        let mut context: Option<(String, &str)> = None;
        // Rows in tree order. An only child continues its parent's column instead of
        // indenting, so indentation counts forks and a long chain stays one column.
        struct Row<'a> {
            id: &'a str,
            indent: usize,
            chained: bool,
        }
        fn walk<'a>(col: &'a Collection, id: &'a str, indent: usize, chained: bool, closed: &HashSet<String>, keep: Option<&HashSet<&str>>, rows: &mut Vec<Row<'a>>) {
            if keep.is_some_and(|k| !k.contains(id)) {
                return;
            }
            rows.push(Row { id, indent, chained });
            if keep.is_none() && closed.contains(id) {
                return;
            }
            let kids = col.children(Some(id));
            let only = kids.len() == 1;
            for k in kids {
                walk(col, k, if only { indent } else { indent + 1 }, only, closed, keep, rows);
            }
        }
        let mut rows = vec![];
        let roots = match hoist {
            Some(h) => vec![h],
            None => col.children(None),
        };
        for r in roots {
            walk(col, r, 0, false, &self.tree_closed, filtering.then_some(&shown), &mut rows);
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for Row { id, indent, chained } in rows {
                let depth = indent;
                let r = &col.replays[id];
                let has_kids = !col.children(Some(id)).is_empty();
                let closed = self.tree_closed.contains(id);
                let row = ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add_space(depth as f32 * 14.0);
                    if has_kids {
                        if ui.add(egui::Button::new(if closed { "⏵" } else { "⏷" }).frame(false).small()).clicked() {
                            toggled = Some(id.to_string());
                        }
                    } else {
                        ui.add_space(14.0);
                    }
                    let st = col.tree_status(id);
                    status_dot(ui, st).on_hover_text(st.label());
                    let mut title = RichText::new(r.title());
                    if filtering && !matches.contains(id) {
                        title = title.weak();
                    }
                    if self.edited.contains(id) {
                        title = title.italics().color(Color32::from_rgb(240, 210, 90));
                    }
                    let sel = self.selected.as_deref() == Some(id);
                    let resp = match &mut self.renaming {
                        Some((rid, text, focus)) if rid == id => {
                            let w = (ui.available_width() - 50.0).max(80.0);
                            let mut out = egui::TextEdit::singleline(text).id_salt("tree_rename").desired_width(w).show(ui);
                            if *focus {
                                // Start with the whole title selected, as IDEs do.
                                *focus = false;
                                out.response.response.request_focus();
                                let n = text.chars().count();
                                out.state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(n))));
                                out.state.store(ui.ctx(), out.response.response.id);
                            } else if out.response.response.lost_focus() {
                                // Enter or clicking away keeps the new title; Escape cancels.
                                let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                rename_done = Some((id.to_string(), (!cancel).then(|| text.trim().to_string())));
                            }
                            out.response.response
                        }
                        _ => ui.selectable_label(sel, title),
                    };
                    let frames = r.frames().unwrap_or(0);
                    ui.label(RichText::new(format!("{frames}f")).weak().small());
                    if r.is_poked() {
                        ui.label(RichText::new("poke").small().color(Color32::from_rgb(200, 120, 220))).on_hover_text("Has [WRITE] lines: memory is changed directly");
                    }
                    let resp = resp.on_hover_ui(|ui| {
                        ui.label(RichText::new(r.title()).strong());
                        ui.label(format!("id {id}"));
                        ui.label(format!("{frames} frames, status {}", st.label()));
                        let tags: Vec<&str> = r.get("tags").map(|t| t.split_whitespace().collect()).unwrap_or_default();
                        if !tags.is_empty() {
                            ui.label(format!("tags: {}", tags.join(", ")));
                        }
                        if self.edited.contains(id) {
                            ui.label("edited, not saved");
                        }
                    });
                    if resp.double_clicked() {
                        double = Some(id.to_string());
                    } else if resp.clicked() {
                        clicked = Some(id.to_string());
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Play to here").on_hover_text("Play through to the end of this node").clicked() {
                            context = Some((id.to_string(), "playto"));
                        }
                        if ui.button("New child at end").clicked() {
                            context = Some((id.to_string(), "child"));
                        }
                        if ui.button("Properties…").clicked() {
                            context = Some((id.to_string(), "props"));
                        }
                        if ui.button("Delete…").on_hover_text("Delete this node and everything below it").clicked() {
                            context = Some((id.to_string(), "delete"));
                        }
                        ui.separator();
                        if ui.add_enabled(hoist != Some(id), egui::Button::new("Focus here")).on_hover_text("Show only this node and what is below it").clicked() {
                            context = Some((id.to_string(), "hoist"));
                        }
                        if hoist.is_some() && ui.button("Show whole tree").clicked() {
                            context = Some((id.to_string(), "unhoist"));
                        }
                    });
                });
                if chained {
                    // A short line up to the parent row marks a node that continues its parent's chain.
                    let rect = row.response.rect;
                    let x = rect.left() + depth as f32 * 14.0 + 7.0;
                    let gap = ui.spacing().item_spacing.y;
                    ui.painter().line_segment([egui::pos2(x, rect.top() - gap - 3.0), egui::pos2(x, rect.top() + 4.0)], egui::Stroke::new(1.5, Color32::from_gray(90)));
                }
            }
        });
        match new_hoist {
            Some(Some(h)) => {
                self.tree_closed.remove(&h);
                self.hoist = Some(h);
            }
            Some(None) => self.hoist = None,
            None => {}
        }
        if let Some(t) = toggled {
            if !self.tree_closed.remove(&t) {
                self.tree_closed.insert(t);
            }
        }
        if let Some(id) = clicked {
            if self.selected.as_deref() != Some(id.as_str()) {
                self.select(&id);
            }
        }
        if let Some(id) = double {
            self.play_node(&id);
        }
        if let Some((id, title)) = rename_done {
            self.renaming = None;
            // The Enter that ended the rename must not start another one.
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            let changed = title.as_ref().is_some_and(|t| !t.is_empty() && self.col.as_ref().and_then(|c| c.replays.get(&id)).is_some_and(|r| r.title() != t));
            if changed {
                self.remember("Rename", &[&id]);
            }
            if let (Some(t), Some(col)) = (title.filter(|_| changed), &mut self.col) {
                if let Some(r) = col.replays.get_mut(&id) {
                    {
                        r.set("title", &t);
                        self.edited.insert(id);
                    }
                }
            }
        }
        match context {
            Some((id, "child")) => {
                if self.selected.as_deref() != Some(id.as_str()) {
                    self.select(&id);
                }
                self.new_child(&id);
            }
            Some((id, "playto")) => self.play_to(&id),
            Some((id, "hoist")) => {
                self.tree_closed.remove(&id);
                self.hoist = Some(id);
            }
            Some((_, "unhoist")) => self.hoist = None,
            Some((id, "delete")) => self.delete_dialog = Some(id),
            Some((id, _)) => self.open_node_dialog(&id),
            None => {}
        }
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let has = self.pb.is_some();
            ui.add_enabled_ui(has, |ui| {
                if ui.button("⏮").on_hover_text("Go to start (Home)").clicked() {
                    self.playing = false;
                    self.seek(0);
                    self.roll.follow_to(0);
                }
                if ui.button("⏴").on_hover_text("Step back (←)").clicked() {
                    self.step_by(-1);
                }
                if ui.button(if self.playing { "⏸" } else { "▶" }).on_hover_text("Play / pause (Space)").clicked() {
                    self.toggle_play();
                }
                if ui.button("⏵").on_hover_text("Step forward (→)").clicked() {
                    self.step_by(1);
                }
                if ui.button("⏭").on_hover_text("Go to end (End)").clicked() {
                    self.playing = false;
                    let e = self.pb.as_ref().map_or(0, |p| p.frames.saturating_sub(1));
                    self.seek(e);
                    self.roll.follow_to(e);
                }
                ui.separator();
                for (sp, label) in [(1, "1×"), (2, "2×"), (4, "4×"), (8, "8×"), (0, "MAX")] {
                    if ui.selectable_label(self.speed == sp, label).on_hover_text("Playback speed").clicked() {
                        self.speed = sp;
                        self.clock = 0.0;
                    }
                }
                ui.separator();
                ui.checkbox(&mut self.auto_continue, "continue").on_hover_text("At the end of a node with exactly one child, and no unsaved changes in either, keep playing into the child");
            });
        });
        ui.horizontal(|ui| {
            // Fixed height: the row is empty while a load runs, and the preview must not move.
            ui.set_min_height(20.0);
            if let (Some(pb), Some(col)) = (&self.pb, &self.col) {
                let shown = pb.shown().unwrap_or(0);
                ui.monospace(format!("frame {shown:>5} / {}", pb.frames.saturating_sub(1)));
                ui.label(RichText::new(format!("power-on frame {}", pb.start_frame + shown)).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(m) = pb.checkpoint_miss {
                        ui.label(RichText::new(format!("checkpoint {m} differs")).color(Color32::from_rgb(230, 90, 80)));
                    }
                    match self.end_matches() {
                        Some(true) => ui.label(RichText::new("✔ end hash matches").color(Color32::from_rgb(92, 184, 92))),
                        Some(false) if col.replays[&pb.id].get("end-hash").is_none() => ui.label(RichText::new("end hash not recorded yet").color(Color32::from_gray(160))),
                        Some(false) => ui.label(RichText::new("✖ end hash differs").color(Color32::from_rgb(230, 90, 80))),
                        None => ui.label(RichText::new("play to the end to check the hash").weak()),
                    };
                });
            }
        });
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        self.transport(ui);
        ui.separator();
        let avail = ui.available_rect_before_wrap();
        ui.painter().rect_filled(avail, 0.0, Color32::from_gray(12));
        if let Some(err) = &self.core_error {
            ui.scope_builder(egui::UiBuilder::new().max_rect(avail.shrink(24.0)), |ui| {
                ui.label(RichText::new(err).color(Color32::from_rgb(240, 160, 60)));
            });
            return;
        }
        let Some(tex) = &self.texture else { return };
        let [w, h] = tex.size();
        let disp_w = if self.aspect > 0.1 { h as f32 * self.aspect } else { w as f32 };
        let size = egui::vec2(disp_w, h as f32);
        let mut scale = (avail.width() / size.x).min(avail.height() / size.y);
        if scale >= 2.0 {
            scale = scale.floor();
        }
        let rect = egui::Rect::from_center_size(avail.center(), size * scale);
        ui.painter().image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        ui.allocate_rect(avail, egui::Sense::hover());
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some((m, err)) = &self.message {
                let t = RichText::new(m).small();
                ui.label(if *err { t.color(Color32::from_rgb(230, 90, 80)) } else { t.weak() });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Fixed slot for the progress bar, so nothing moves when it appears.
                let slot = egui::vec2(260.0, 16.0);
                match self.job_progress() {
                    Some((f, text)) => {
                        ui.add(egui::ProgressBar::new(f.clamp(0.0, 1.0)).desired_width(slot.x).desired_height(slot.y).text(RichText::new(text).small()).corner_radius(3.0));
                    }
                    None => {
                        ui.allocate_space(slot);
                    }
                }
                if let Some(col) = &self.col {
                    ui.label(RichText::new(format!("{} {}", col.config.core.name, col.config.core.build)).small().weak());
                    if !self.edited.is_empty() {
                        ui.label(RichText::new(format!("{} unsaved", self.edited.len())).small().color(Color32::from_rgb(240, 210, 90)));
                    }
                }
            });
        });
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        use egui::{Key, Modifiers};
        let (space, right, left, home, end, save) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Space),
                i.consume_key(Modifiers::NONE, Key::ArrowRight),
                i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                i.consume_key(Modifiers::NONE, Key::Home),
                i.consume_key(Modifiers::NONE, Key::End),
                i.consume_key(Modifiers::COMMAND, Key::S),
            )
        });
        let add_event = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::E));
        // Redo first: Cmd+Z alone would also match Cmd+Shift+Z.
        let (undo, redo, mut cut, mut copy, mut paste) = ctx.input_mut(|i| {
            let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y);
            let undo = i.consume_key(Modifiers::COMMAND, Key::Z);
            (undo, redo, i.consume_key(Modifiers::COMMAND, Key::X), i.consume_key(Modifiers::COMMAND, Key::C), i.consume_key(Modifiers::COMMAND, Key::V))
        });
        // Some platforms deliver the clipboard keys as events instead of key presses.
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                egui::Event::Cut => {
                    cut = true;
                    false
                }
                egui::Event::Copy => {
                    copy = true;
                    false
                }
                egui::Event::Paste(_) => {
                    paste = true;
                    false
                }
                _ => true,
            })
        });
        if redo {
            self.redo();
        } else if undo {
            self.undo();
        }
        if cut {
            self.cut();
        } else if copy {
            self.copy();
        }
        if paste {
            self.paste();
        }
        let (rename, draw, select) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::F2),
                i.consume_key(Modifiers::NONE, Key::D),
                i.consume_key(Modifiers::NONE, Key::S),
            )
        });
        if draw {
            self.roll.mode = roll::Mode::Draw;
        }
        if select {
            self.roll.mode = roll::Mode::Select;
        }
        if rename && self.node_dialog.is_none() && self.frame_dialog.is_none() && self.renaming.is_none() {
            if let (Some(id), Some(col)) = (&self.selected, &self.col) {
                let title = col.replays[id].title().to_string();
                self.renaming = Some((id.clone(), title, true));
                // Make sure the row is visible.
                if self.hoist.as_ref().is_some_and(|h| col.chain(id).is_ok_and(|c| !c.contains(&h.as_str()))) {
                    self.hoist = None;
                }
            }
        }
        if add_event && self.pb.is_some() {
            let f = self.pb.as_ref().and_then(|p| p.shown()).unwrap_or(0);
            self.open_frame_dialog(f, "[EVENT] ");
        }
        if space {
            self.toggle_play();
        }
        if right {
            self.step_by(1);
        }
        if left {
            self.step_by(-1);
        }
        if home {
            self.playing = false;
            self.seek(0);
            self.roll.follow_to(0);
        }
        if end {
            self.playing = false;
            let e = self.pb.as_ref().map_or(0, |p| p.frames.saturating_sub(1));
            self.seek(e);
            self.roll.follow_to(e);
        }
        if save {
            self.save();
        }
    }
}

thread_local! {
    static CTX: std::cell::RefCell<Option<egui::Context>> = const { std::cell::RefCell::new(None) };
}

fn egui_ctx() -> egui::Context {
    CTX.with(|c| c.borrow().clone().expect("egui context"))
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
        self.shortcuts(&ctx);
        if let Some(t) = self.ctx_copy.take() {
            ctx.copy_text(t);
        }
        let dt = ctx.input(|i| i.unstable_dt).min(0.1) as f64;
        self.work();
        if self.job.is_some() {
            ctx.request_repaint();
        }
        self.tick(dt);
        if self.playing {
            ctx.request_repaint();
        }
        let title = match &self.col {
            Some(c) => format!("emucov: {}{}", c.config.name, if self.edited.is_empty() { "" } else { " *" }),
            None => "emucov".into(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").exact_size(24.0).show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("tree").resizable(true).default_size(300.0).size_range(180.0..=600.0).show(ui, |ui| self.tree(ui));
        let mut action = None;
        egui::Panel::bottom("roll").resizable(true).default_size(300.0).size_range(120.0..=900.0).show(ui, |ui| {
            if let (Some(col), Some(id)) = (&self.col, &self.selected) {
                let shown = self.pb.as_ref().and_then(|p| p.shown());
                action = self.roll.show(ui, col, id, shown, self.playing);
            } else {
                ui.label("Select a replay to see its inputs.");
            }
        });
        egui::CentralPanel::default().show(ui, |ui| self.preview(ui));
        self.dialogs(&ctx);

        match action {
            Some(roll::Action::Seek(f)) => {
                self.playing = false;
                self.seek(f);
            }
            Some(roll::Action::Edit { edits, from }) => self.apply_edit("Edit notes", edits, from),
            Some(roll::Action::OpenFrame(f)) => self.open_frame_dialog(f, ""),
            None => {}
        }
    }
}
