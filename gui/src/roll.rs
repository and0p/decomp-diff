//! The piano roll: time runs left to right, one row per button, rows grouped by
//! controller. Groups that the replay uses are highlighted and open; unused
//! groups are dimmed and collapsed. Only binary inputs (buttons and keys) get
//! rows for now; other input lines show as ticks on their group's header.
//!
//! Spans are worked out once per replay edit and drawn only where visible, so
//! long replays with many presses stay cheap to draw.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind};
use emucov::collection::Collection;
use emucov::format::{self, Command, InputKey, DEVICE_JOYPAD, DEVICE_KEYBOARD, DEVICE_MOUSE};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const LABEL_W: f32 = 132.0;
const RULER_H: f32 = 20.0;
const MARK_H: f32 = 18.0;
/// Longest marker label drawn in the lane, in characters.
const LABEL_CHARS: usize = 40;
const GROUP_H: f32 = 20.0;
const ROW_H: f32 = 15.0;
/// Joypad rows in display order: d-pad, SELECT, START, B, A, then the rest.
const JOYPAD_ORDER: [u32; 16] = [4, 5, 6, 7, 2, 3, 0, 8, 1, 9, 10, 11, 12, 13, 14, 15];
const JOYPAD_BASIC: usize = 8;

pub enum Action {
    Seek(u64),
    /// Open the editor for the non-input lines on this frame.
    OpenFrame(u64),
    /// New spans for one or more buttons; `from` is the first frame whose input changed.
    Edit { edits: Vec<(InputKey, Vec<(u64, u64)>)>, from: u64 },
}

/// What a drag on the button rows does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Drag to press or release frames.
    Draw,
    /// Select whole presses, move them, and drag their edges.
    Select,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SelOp {
    Replace,
    Add,
    Remove,
}

/// A press is named by its button and first frame.
type PressId = (InputKey, u64);

/// A drag in select mode. Frame positions are fractional frames, so the
/// view can scroll under a drag without losing its anchor.
enum Gesture {
    /// Moving the selected presses sideways. `click` is the press under the pointer,
    /// which becomes the only selection if the pointer never moves.
    Move { anchor: f32, delta: i64, click: Option<PressId> },
    /// Dragging one edge of one press.
    Resize { key: InputKey, span: (u64, u64), left: bool, to: u64 },
    /// Selecting by area: corners in (frame, content y).
    Marquee { a: (f32, f32), b: (f32, f32), op: SelOp },
}

struct Row {
    key: InputKey,
    label: String,
    spans: Vec<(u64, u64)>,
}

struct Group {
    port: u32,
    device: u32,
    name: String,
    rows: Vec<Row>,
    /// All rows' spans merged, drawn on the header.
    union: Vec<(u64, u64)>,
    /// Frames with non-binary input lines (axes, pointer positions).
    other: Vec<u64>,
    presses: usize,
}

impl Group {
    fn used(&self) -> bool {
        self.presses > 0 || !self.other.is_empty()
    }
}

struct Marker {
    frame: u64,
    glyph: &'static str,
    color: Color32,
    text: String,
    /// Short text shown in the lane.
    label: String,
}

struct Model {
    id: String,
    gen: u64,
    all_buttons: bool,
    frames: u64,
    groups: Vec<Group>,
    markers: Vec<Marker>,
}

struct Drag {
    group: usize,
    row: usize,
    anchor: u64,
    current: u64,
    press: bool,
}

pub struct RollState {
    pub px_per_frame: f32,
    /// Frame at the left edge of the timeline.
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub follow: bool,
    pub all_buttons: bool,
    /// Show marker text in the lane (otherwise only symbols).
    pub labels: bool,
    /// Rows in the marker lane; more rows leave more room for labels.
    pub lane_rows: usize,
    open: HashMap<(u32, u32), bool>,
    gen: u64,
    model: Option<Model>,
    drag: Option<Drag>,
    pub mode: Mode,
    /// Selected presses (select mode).
    sel: BTreeSet<PressId>,
    gesture: Option<Gesture>,
    scrubbing: bool,
    jump_to: Option<u64>,
}

impl Default for RollState {
    fn default() -> Self {
        RollState { px_per_frame: 4.0, scroll_x: 0.0, scroll_y: 0.0, follow: true, all_buttons: false, labels: true, lane_rows: 1, open: HashMap::new(), gen: 0, model: None, drag: None, mode: Mode::Draw, sel: BTreeSet::new(), gesture: None, scrubbing: false, jump_to: None }
    }
}

fn port_color(port: u32, device: u32) -> Color32 {
    if device != DEVICE_JOYPAD {
        return Color32::from_rgb(235, 150, 60);
    }
    match port {
        0 => Color32::from_rgb(80, 150, 255),
        1 => Color32::from_rgb(240, 90, 100),
        2 => Color32::from_rgb(90, 200, 120),
        _ => Color32::from_rgb(190, 120, 240),
    }
}

fn device_name(d: u32) -> &'static str {
    match d {
        DEVICE_JOYPAD => "Joypad",
        DEVICE_MOUSE => "Mouse",
        DEVICE_KEYBOARD => "Keyboard",
        format::DEVICE_LIGHTGUN => "Lightgun",
        format::DEVICE_ANALOG => "Analog",
        format::DEVICE_POINTER => "Pointer",
        _ => "Device",
    }
}

fn key_label(key: InputKey) -> String {
    match key.device {
        DEVICE_JOYPAD if key.id < 16 => format::JOYPAD[key.id as usize].to_string(),
        DEVICE_MOUSE => format::mouse_name(key.id).unwrap_or("?").to_string(),
        _ => {
            let t = format::input_text(key, 1);
            let t = t.strip_suffix(" 1").unwrap_or(&t);
            t.split_once(' ').map_or(t, |(_, rest)| rest).to_string()
        }
    }
}

pub fn merge(mut v: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    v.sort();
    let mut out: Vec<(u64, u64)> = vec![];
    for (a, b) in v {
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn build_model(col: &Collection, id: &str, gen: u64, all_buttons: bool) -> Model {
    let r = &col.replays[id];
    let frames = r.frames().unwrap_or(0);
    let mut used: BTreeSet<InputKey> = BTreeSet::new();
    let mut other: BTreeMap<(u32, u32), Vec<u64>> = BTreeMap::new();
    let mut markers = vec![];
    let mut groups_wanted: BTreeSet<(u32, u32)> = BTreeSet::new();
    // Joypads on the collection's ports, and always at least two of them.
    for p in &col.config.ports {
        if let Ok(d) = format::device_from_name(&p.device) {
            if d & 0xff == DEVICE_JOYPAD {
                groups_wanted.insert((p.port, DEVICE_JOYPAD));
            }
        }
    }
    groups_wanted.insert((0, DEVICE_JOYPAD));
    groups_wanted.insert((1, DEVICE_JOYPAD));
    for l in &r.lines {
        match &l.command {
            Command::Input { key, .. } => {
                groups_wanted.insert((key.port, key.device));
                if format::is_binary(*key) {
                    used.insert(*key);
                } else {
                    other.entry((key.port, key.device)).or_default().push(l.frame);
                }
            }
            Command::Device { port, device } => markers.push(Marker { frame: l.frame, glyph: "◆", color: Color32::from_rgb(120, 200, 230), text: format!("[DEVICE] P{} → {device}", port + 1), label: format!("P{} dev {device}", port + 1) }),
            Command::Write { region, address, bytes } => {
                let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
                markers.push(Marker { frame: l.frame, glyph: "✏", color: Color32::from_rgb(200, 120, 220), text: format!("[WRITE] {region} 0x{address:X} {}", hex.join(" ")), label: format!("{region} {address:X}={}", hex.join(" ")) })
            }
            Command::Event { text, .. } => markers.push(Marker { frame: l.frame, glyph: "⚑", color: Color32::from_rgb(240, 210, 90), text: format!("[EVENT] {text}"), label: text.clone() }),
            Command::Reset => markers.push(Marker { frame: l.frame, glyph: "⟲", color: Color32::from_rgb(230, 90, 80), text: "[RESET]".into(), label: "reset".into() }),
            Command::Screenshot => markers.push(Marker { frame: l.frame, glyph: "▣", color: Color32::from_gray(200), text: "[SCREENSHOT]".into(), label: String::new() }),
            Command::Rng { name, count } => markers.push(Marker { frame: l.frame, glyph: "🎲", color: Color32::from_gray(200), text: format!("[RNG] {name} {count}"), label: format!("rng {name}") }),
        }
    }
    for (f, h) in r.checkpoints().unwrap_or_default() {
        markers.push(Marker { frame: f, glyph: "✔", color: Color32::from_rgb(92, 184, 92), text: format!("checkpoint {h}"), label: String::new() });
    }
    let mut groups = vec![];
    for (port, device) in groups_wanted {
        let ids: Vec<u32> = match device {
            DEVICE_JOYPAD => JOYPAD_ORDER.iter().enumerate().filter(|(i, id)| all_buttons || *i < JOYPAD_BASIC || used.contains(&InputKey { port, device, index: 0, id: **id })).map(|(_, id)| *id).collect(),
            DEVICE_MOUSE => {
                let mut v = vec![2, 3, 6];
                v.extend(used.iter().filter(|k| k.port == port && k.device == device && !v.contains(&k.id)).map(|k| k.id).collect::<Vec<_>>());
                v
            }
            _ => used.iter().filter(|k| k.port == port && k.device == device).map(|k| k.id).collect(),
        };
        let mut rows = vec![];
        for id in ids {
            // Keyboard and other devices may use index; take whichever keys appear.
            let keys: Vec<InputKey> = {
                let found: Vec<InputKey> = used.iter().filter(|k| k.port == port && k.device == device && k.id == id).copied().collect();
                if found.is_empty() {
                    vec![InputKey { port, device, index: 0, id }]
                } else {
                    found
                }
            };
            for key in keys {
                let spans = r.binary_spans(key);
                rows.push(Row { key, label: key_label(key), spans });
            }
        }
        let presses = rows.iter().map(|r| r.spans.len()).sum();
        let union = merge(rows.iter().flat_map(|r| r.spans.iter().copied()).collect());
        let mut oth = other.remove(&(port, device)).unwrap_or_default();
        oth.dedup();
        groups.push(Group { port, device, name: format!("P{} {}", port + 1, device_name(device)), rows, union, other: oth, presses });
    }
    markers.sort_by_key(|m| m.frame);
    Model { id: id.to_string(), gen, all_buttons, frames, groups, markers }
}

/// Spans overlapping `[f0, f1)`, found by binary search.
fn visible(spans: &[(u64, u64)], f0: u64, f1: u64) -> &[(u64, u64)] {
    let a = spans.partition_point(|s| s.1 <= f0);
    let b = spans.partition_point(|s| s.0 < f1);
    &spans[a..b.max(a)]
}

fn row_of(m: &Model, key: InputKey) -> Option<(usize, usize)> {
    m.groups.iter().enumerate().find_map(|(gi, g)| g.rows.iter().position(|r| r.key == key).map(|ri| (gi, ri)))
}

/// The span of `spans` that contains frame `f`.
fn span_at(spans: &[(u64, u64)], f: u64) -> Option<(u64, u64)> {
    spans.iter().copied().find(|s| s.0 <= f && f < s.1)
}

fn tick_step(px_per_frame: f32) -> u64 {
    for s in [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1200, 1800, 3600, 7200, 18000, 36000] {
        if s as f32 * px_per_frame >= 64.0 {
            return s;
        }
    }
    72000
}

enum Item {
    Header(usize),
    Row(usize, usize),
}

impl RollState {
    /// The selected notes with their spans, in the node the roll last drew.
    pub fn selected_notes(&self) -> Vec<(InputKey, (u64, u64))> {
        let Some(m) = &self.model else { return vec![] };
        self.sel
            .iter()
            .filter_map(|(key, start)| {
                let (gi, ri) = row_of(m, *key)?;
                m.groups[gi].rows[ri].spans.iter().find(|sp| sp.0 == *start).map(|sp| (*key, *sp))
            })
            .collect()
    }

    /// Replaces the selection (notes named by button and first frame).
    pub fn set_selection(&mut self, notes: impl IntoIterator<Item = (InputKey, u64)>) {
        self.sel = notes.into_iter().collect();
    }

    pub fn invalidate(&mut self) {
        self.gen += 1;
    }

    pub fn forget(&mut self) {
        self.model = None;
        self.open.clear();
        self.scroll_y = 0.0;
        self.invalidate();
    }

    pub fn zoom(&mut self, factor: f32) {
        self.px_per_frame = (self.px_per_frame * factor).clamp(0.02, 48.0);
    }

    /// Makes sure `frame` is in view on the next draw.
    pub fn follow_to(&mut self, frame: u64) {
        self.jump_to = Some(frame);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, col: &Collection, id: &str, playhead: Option<u64>, playing: bool) -> Option<Action> {
        let stale = match &self.model {
            Some(m) => m.id != id || m.gen != self.gen || m.all_buttons != self.all_buttons,
            None => true,
        };
        if stale {
            if self.model.as_ref().is_some_and(|m| m.id != id) {
                self.scroll_x = 0.0;
                self.drag = None;
                self.gesture = None;
                self.sel.clear();
            }
            let m = build_model(col, id, self.gen, self.all_buttons);
            // Keep only selected presses that still exist after the edit.
            self.sel.retain(|(key, start)| row_of(&m, *key).is_some_and(|(g, r)| m.groups[g].rows[r].spans.iter().any(|s| s.0 == *start)));
            self.model = Some(m);
        }
        let model = self.model.take().unwrap();
        let action = self.draw(ui, &model, playhead, playing);
        self.model = Some(model);
        action
    }

    fn is_open(&self, g: &Group) -> bool {
        *self.open.get(&(g.port, g.device)).unwrap_or(&g.used())
    }

    fn draw(&mut self, ui: &mut egui::Ui, m: &Model, playhead: Option<u64>, playing: bool) -> Option<Action> {
        let mut action = None;
        // Toolbar.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Inputs").strong());
            ui.selectable_value(&mut self.mode, Mode::Draw, "Draw").on_hover_text("Drag in a button row to press or release frames (D)");
            ui.selectable_value(&mut self.mode, Mode::Select, "Select").on_hover_text(
                "Click a press to select it; Shift adds, Alt+Shift removes. Drag empty space to select an area. Drag presses sideways to move them, or drag an edge to resize. Delete removes. (S)",
            );
            if self.mode == Mode::Select && !self.sel.is_empty() {
                ui.label(egui::RichText::new(format!("{} selected", self.sel.len())).small());
            }
            ui.separator();
            let used: Vec<&str> = m.groups.iter().filter(|g| g.used()).map(|g| g.name.as_str()).collect();
            ui.label(egui::RichText::new(if used.is_empty() { "no inputs in this replay".to_string() } else { format!("uses {}", used.join(", ")) }).weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(&mut self.all_buttons, "all buttons");
                ui.checkbox(&mut self.labels, "labels");
                if ui.small_button("+").on_hover_text("More marker rows").clicked() {
                    self.lane_rows = (self.lane_rows + 1).min(8);
                }
                ui.label(format!("{}", self.lane_rows)).on_hover_text("Marker rows");
                if ui.small_button("−").on_hover_text("Fewer marker rows").clicked() {
                    self.lane_rows = self.lane_rows.saturating_sub(1).max(1);
                }
                ui.label(egui::RichText::new("marker rows").weak());
                ui.separator();
                ui.checkbox(&mut self.follow, "follow");
                if ui.small_button("fit").on_hover_text("Fit the whole replay").clicked() {
                    let w = (ui.available_width() + 200.0 - LABEL_W).max(100.0);
                    self.px_per_frame = (w / m.frames.max(1) as f32).clamp(0.02, 48.0);
                    self.scroll_x = 0.0;
                }
                if ui.small_button("+").clicked() {
                    self.zoom(1.5);
                }
                if ui.small_button("−").clicked() {
                    self.zoom(1.0 / 1.5);
                }
                ui.label(egui::RichText::new(format!("{:.2} px/frame", self.px_per_frame)).weak().small());
            });
        });

        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let tl_left = rect.left() + LABEL_W;
        let tl_w = (rect.right() - tl_left).max(1.0);
        let lane_h = MARK_H * self.lane_rows as f32;
        let rows_top = rect.top() + RULER_H + lane_h;
        let ppf = self.px_per_frame;

        // Layout of rows.
        let mut items: Vec<(f32, f32, Item)> = vec![];
        let mut y = 0.0;
        for (gi, g) in m.groups.iter().enumerate() {
            items.push((y, y + GROUP_H, Item::Header(gi)));
            y += GROUP_H;
            if self.is_open(g) {
                for ri in 0..g.rows.len() {
                    items.push((y, y + ROW_H, Item::Row(gi, ri)));
                    y += ROW_H;
                }
            }
        }
        let content_h = y;
        let view_h = (rect.bottom() - rows_top).max(1.0);

        // Scrolling and zoom.
        let pointer = ui.input(|i| i.pointer.hover_pos());
        if resp.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            if let Some(p) = pointer {
                if zoom != 1.0 && p.x > tl_left {
                    let anchor = self.scroll_x + (p.x - tl_left) / ppf;
                    self.zoom(zoom);
                    self.scroll_x = anchor - (p.x - tl_left) / self.px_per_frame;
                } else if p.x < tl_left {
                    self.scroll_y -= scroll.y;
                } else {
                    self.scroll_x -= (scroll.x + scroll.y) / ppf;
                }
            }
        }
        // While dragging in select mode, holding the pointer near either end of the
        // timeline scrolls it at a steady speed in pixels, so the pace follows the zoom.
        if self.gesture.is_some() {
            if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                let edge = 36.0;
                let (lo, hi) = (tl_left + edge, rect.right() - edge);
                let k = if p.x < lo { -((lo - p.x) / edge).min(1.5) } else if p.x > hi { ((p.x - hi) / edge).min(1.5) } else { 0.0 };
                if k != 0.0 {
                    let dt = ui.input(|i| i.stable_dt).min(0.05);
                    self.scroll_x += k * 500.0 * dt / self.px_per_frame;
                    ui.ctx().request_repaint();
                }
            }
        }
        let ppf = self.px_per_frame;
        let view_frames = tl_w / ppf;
        if let Some(f) = self.jump_to.take() {
            let f = f as f32;
            if f < self.scroll_x || f > self.scroll_x + view_frames {
                self.scroll_x = f - view_frames * 0.1;
            }
        }
        if let (Some(ph), true, true) = (playhead, playing, self.follow) {
            let ph = ph as f32;
            if ph < self.scroll_x || ph > self.scroll_x + view_frames * 0.9 {
                self.scroll_x = ph - view_frames * 0.1;
            }
        }
        self.scroll_x = self.scroll_x.clamp(-view_frames * 0.05, (m.frames as f32 - view_frames * 0.5).max(0.0));
        self.scroll_y = self.scroll_y.clamp(0.0, (content_h - view_h).max(0.0));

        let x_of = |f: f32| tl_left + (f - self.scroll_x) * ppf;
        let frame_at = |x: f32| ((x - tl_left) / ppf + self.scroll_x).floor().max(0.0) as u64;
        let f0 = self.scroll_x.floor().max(0.0) as u64;
        let f1 = ((self.scroll_x + view_frames).ceil().max(0.0) as u64 + 1).min(m.frames);

        // Marker layout: markers on one frame share a box. Boxes go into the first lane
        // row with room for their whole label; a label then runs until the next box in its row.
        let my = rect.top() + RULER_H + MARK_H * 0.5;
        let row_y = |r: usize| rect.top() + RULER_H + MARK_H * (r as f32 + 0.5);
        let mut boxes: Vec<(u64, f32, Vec<usize>)> = vec![];
        let a0 = m.markers.partition_point(|mk| mk.frame < f0);
        for (i, mk) in m.markers.iter().enumerate().skip(a0) {
            if mk.frame > f1 {
                break;
            }
            let x = x_of(mk.frame as f32 + 0.5 * (ppf >= 4.0) as u8 as f32).max(tl_left + 6.0);
            match boxes.last_mut() {
                Some(b) if b.0 == mk.frame || x - b.1 < 3.0 => b.2.push(i),
                _ => boxes.push((mk.frame, x, vec![i])),
            }
        }
        let font = FontId::proportional(11.0);
        let measure = |t: &str| ui.painter().layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x;
        // Full label text and colour for each box.
        let texts: Vec<(String, Color32)> = boxes
            .iter()
            .map(|(_, _, ixs)| {
                if !self.labels {
                    return (String::new(), Color32::WHITE);
                }
                let mut named: Vec<&Marker> = ixs.iter().map(|i| &m.markers[*i]).filter(|mk| !mk.label.is_empty()).collect();
                // An author's event text says the most, so it goes first.
                named.sort_by_key(|mk| mk.glyph != "⚑");
                let mut text = named.first().map(|mk| mk.label.clone()).unwrap_or_default();
                if ixs.len() > 1 {
                    text = format!("({}) {text}", ixs.len()).trim_end().to_string();
                }
                if text.chars().count() > LABEL_CHARS {
                    text = text.chars().take(LABEL_CHARS - 1).collect::<String>() + "…";
                }
                (text, named.first().map_or(m.markers[ixs[0]].color, |mk| mk.color))
            })
            .collect();
        let rows = self.lane_rows.max(1);
        let mut row_end = vec![f32::NEG_INFINITY; rows];
        let mut box_row = vec![0usize; boxes.len()];
        for (bi, (_, x, _)) in boxes.iter().enumerate() {
            let want = if texts[bi].0.is_empty() { 8.0 } else { 14.0 + measure(&texts[bi].0) };
            let r = (0..rows).find(|r| row_end[*r] <= x - 6.0).unwrap_or_else(|| (0..rows).min_by(|a, b| row_end[*a].total_cmp(&row_end[*b])).unwrap());
            box_row[bi] = r;
            row_end[r] = x + want;
        }
        let mut marker_rects: Vec<(Rect, usize)> = vec![];
        let mut marker_labels: Vec<(f32, f32, String, Color32)> = vec![];
        for (bi, (_, x, _)) in boxes.iter().enumerate() {
            let r = box_row[bi];
            let next_x = boxes.iter().enumerate().skip(bi + 1).find(|(j, _)| box_row[*j] == r).map_or(rect.right(), |(_, b)| b.1);
            let mut right = *x + 7.0;
            let (mut text, c) = texts[bi].clone();
            let avail = next_x - x - 14.0;
            if !text.is_empty() && avail > 14.0 {
                let mut full = measure(&text);
                while full > avail && text.chars().count() > 1 {
                    let n = text.trim_end_matches('…').chars().count();
                    let keep = ((n as f32 * avail / full) as usize).min(n - 1).max(1);
                    text = text.chars().take(keep).collect::<String>().trim_end().to_string() + "…";
                    full = measure(&text);
                    if keep == 1 {
                        break;
                    }
                }
                if full <= avail {
                    marker_labels.push((*x + 8.0, row_y(r), text, c));
                    right = *x + 10.0 + full;
                }
            }
            let top = rect.top() + RULER_H + MARK_H * r as f32;
            marker_rects.push((Rect::from_min_max(Pos2::new(*x - 6.0, top), Pos2::new(right, top + MARK_H)), bi));
        }
        let marker_at = |p: Pos2| marker_rects.iter().find(|(r, _)| r.contains(p)).map(|(_, bi)| *bi);

        // Pointer interaction.
        let (pressed, down, released) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.primary_released()));
        let hit = |p: Pos2| -> Option<usize> {
            if p.y < rows_top {
                return None;
            }
            let yy = p.y - rows_top + self.scroll_y;
            items.iter().position(|(a, b, _)| yy >= *a && yy < *b)
        };
        // ---- select mode ----
        let ff_at = |x: f32| (x - tl_left) / ppf + self.scroll_x;
        let mods = ui.input(|i| i.modifiers);
        let op = if mods.shift && mods.alt {
            SelOp::Remove
        } else if mods.shift {
            SelOp::Add
        } else {
            SelOp::Replace
        };
        // Edges can be dragged only while at most one note is selected.
        let edges_ok = self.sel.len() <= 1;
        // The press under `p`, and whether `p` is on its left (true) or right (false) edge.
        let press_at = |p: Pos2| -> Option<(InputKey, (u64, u64), Option<bool>)> {
            let ix = hit(p)?;
            let Item::Row(gi, ri) = items[ix].2 else { return None };
            if p.x < tl_left {
                return None;
            }
            let row = &m.groups[gi].rows[ri];
            let f = ff_at(p.x);
            let lo = (f - 8.0 / ppf).floor().max(0.0) as u64;
            let hi = (f + 8.0 / ppf).ceil().max(0.0) as u64 + 1;
            let mut body = None;
            let edges = edges_ok;
            for &sp in visible(&row.spans, lo, hi) {
                let (x0, x1) = (x_of(sp.0 as f32), x_of(sp.1 as f32).max(x_of(sp.0 as f32) + 1.5));
                // Wide notes have a thin grab zone on each edge (mostly outside the note, so a
                // drag on the note moves it); narrow ones have it just outside.
                let wide = x1 - x0 >= 16.0;
                if edges && ((wide && p.x >= x0 - 3.0 && p.x <= x0 + 1.5) || (!wide && p.x < x0 && p.x >= x0 - 4.0)) {
                    return Some((row.key, sp, Some(true)));
                }
                if edges && ((wide && p.x >= x1 - 1.5 && p.x <= x1 + 3.0) || (!wide && p.x > x1 && p.x <= x1 + 4.0)) {
                    return Some((row.key, sp, Some(false)));
                }
                if p.x >= x0 && p.x < x1 {
                    body = Some((row.key, sp, None));
                }
            }
            body
        };
        // Selected presses with their current spans.
        let sel_spans = |sel: &BTreeSet<PressId>| -> Vec<(InputKey, (u64, u64))> {
            sel.iter()
                .filter_map(|(key, start)| {
                    let (gi, ri) = row_of(m, *key)?;
                    m.groups[gi].rows[ri].spans.iter().find(|sp| sp.0 == *start).map(|sp| (*key, *sp))
                })
                .collect()
        };
        // Presses inside a marquee given as (frame, content y) corners.
        let in_area = |a: (f32, f32), b: (f32, f32)| -> Vec<PressId> {
            let (fa, fb) = (a.0.min(b.0), a.0.max(b.0));
            let (ya, yb) = (a.1.min(b.1), a.1.max(b.1));
            let mut out = vec![];
            for (y0, y1, item) in &items {
                if let Item::Row(gi, ri) = *item {
                    if *y1 > ya && *y0 < yb {
                        let row = &m.groups[gi].rows[ri];
                        for sp in &row.spans {
                            if (sp.0 as f32) < fb && (sp.1 as f32) > fa {
                                out.push((row.key, sp.0));
                            }
                        }
                    }
                }
            }
            out
        };
        let header_at = |p: Pos2| hit(p).is_some_and(|ix| matches!(items[ix].2, Item::Header(_)));
        if pressed && resp.hovered() {
            if let Some(p) = pointer {
                if let Some(bi) = marker_at(p) {
                    action = Some(Action::OpenFrame(boxes[bi].0));
                } else if p.y < rows_top && p.x >= tl_left {
                    self.scrubbing = true;
                } else if self.mode == Mode::Select && p.x >= tl_left && !header_at(p) {
                    match press_at(p) {
                        Some((key, sp, Some(left))) => {
                            self.gesture = Some(Gesture::Resize { key, span: sp, left, to: if left { sp.0 } else { sp.1 } });
                        }
                        Some((key, sp, None)) => {
                            let id = (key, sp.0);
                            let anchor = ff_at(p.x);
                            match op {
                                SelOp::Remove => {
                                    self.sel.remove(&id);
                                }
                                SelOp::Add => {
                                    self.sel.insert(id);
                                    self.gesture = Some(Gesture::Move { anchor, delta: 0, click: None });
                                }
                                SelOp::Replace => {
                                    // Pressing on an already selected press keeps the selection,
                                    // so a group can be dragged; a plain click then selects just it.
                                    let was = self.sel.contains(&id);
                                    if !was {
                                        self.sel.clear();
                                        self.sel.insert(id);
                                    }
                                    self.gesture = Some(Gesture::Move { anchor, delta: 0, click: was.then_some(id) });
                                }
                            }
                        }
                        None => {
                            let a = (ff_at(p.x), p.y - rows_top + self.scroll_y);
                            self.gesture = Some(Gesture::Marquee { a, b: a, op });
                        }
                    }
                } else if let Some(ix) = hit(p) {
                    match items[ix].2 {
                        Item::Header(gi) => {
                            let g = &m.groups[gi];
                            if p.x < tl_left {
                                let now = self.is_open(g);
                                self.open.insert((g.port, g.device), !now);
                            } else {
                                self.scrubbing = true;
                            }
                        }
                        Item::Row(gi, ri) if p.x >= tl_left => {
                            let f = frame_at(p.x);
                            if f < m.frames {
                                let on = m.groups[gi].rows[ri].spans.iter().any(|s| s.0 <= f && f < s.1);
                                self.drag = Some(Drag { group: gi, row: ri, anchor: f, current: f, press: !on });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if let Some(p) = pointer.or_else(|| ui.input(|i| i.pointer.interact_pos())) {
            if self.scrubbing {
                let f = frame_at(p.x).min(m.frames.saturating_sub(1));
                if playhead != Some(f) {
                    action = Some(Action::Seek(f));
                }
            }
            if let Some(d) = &mut self.drag {
                d.current = frame_at(p.x).min(m.frames.saturating_sub(1));
            }
        }
        if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
            match &mut self.gesture {
                Some(Gesture::Move { anchor, delta, .. }) => {
                    let spans = sel_spans(&self.sel);
                    let lo = -(spans.iter().map(|(_, sp)| sp.0).min().unwrap_or(0) as i64);
                    let hi = m.frames as i64 - spans.iter().map(|(_, sp)| sp.1).max().unwrap_or(0) as i64;
                    *delta = ((ff_at(p.x) - *anchor).round() as i64).clamp(lo, hi.max(lo));
                }
                Some(Gesture::Resize { span, left, to, .. }) => {
                    let f = ff_at(p.x).round().max(0.0) as u64;
                    *to = if *left { f.min(span.1 - 1) } else { f.clamp(span.0 + 1, m.frames.max(span.0 + 1)) };
                }
                Some(Gesture::Marquee { b, .. }) => {
                    *b = (ff_at(p.x), p.y - rows_top + self.scroll_y);
                }
                None => {}
            }
        }
        if released || !down {
            if let Some(g) = self.gesture.take() {
                match g {
                    Gesture::Move { delta, click, .. } => {
                        if delta == 0 {
                            if let Some(c) = click {
                                self.sel.clear();
                                self.sel.insert(c);
                            }
                        } else {
                            let moved = sel_spans(&self.sel);
                            let keys: BTreeSet<InputKey> = moved.iter().map(|(k, _)| *k).collect();
                            let mut edits = vec![];
                            let mut from = u64::MAX;
                            let mut new_sel = BTreeSet::new();
                            for key in keys {
                                let Some((gi, ri)) = row_of(m, key) else { continue };
                                let mine: Vec<(u64, u64)> = moved.iter().filter(|(k, _)| *k == key).map(|(_, sp)| *sp).collect();
                                let mut spans: Vec<(u64, u64)> = m.groups[gi].rows[ri].spans.iter().copied().filter(|sp| !mine.contains(sp)).collect();
                                for sp in &mine {
                                    let a = (sp.0 as i64 + delta) as u64;
                                    spans.push((a, a + (sp.1 - sp.0)));
                                    from = from.min(sp.0.min(a));
                                }
                                let spans = merge(spans);
                                for sp in &mine {
                                    let a = (sp.0 as i64 + delta) as u64;
                                    if let Some(t) = span_at(&spans, a) {
                                        new_sel.insert((key, t.0));
                                    }
                                }
                                edits.push((key, spans));
                            }
                            if !edits.is_empty() {
                                self.sel = new_sel;
                                action = Some(Action::Edit { edits, from });
                            }
                        }
                    }
                    Gesture::Resize { key, span, left, to } => {
                        let new = if left { (to, span.1) } else { (span.0, to) };
                        if let (true, Some((gi, ri))) = (new != span, row_of(m, key)) {
                            let mut spans: Vec<(u64, u64)> = m.groups[gi].rows[ri].spans.iter().copied().filter(|sp| *sp != span).collect();
                            spans.push(new);
                            let spans = merge(spans);
                            let from = if left { span.0.min(to) } else { span.1.min(to) };
                            self.sel.remove(&(key, span.0));
                            if let Some(t) = span_at(&spans, new.0) {
                                self.sel.insert((key, t.0));
                            }
                            action = Some(Action::Edit { edits: vec![(key, spans)], from });
                        }
                    }
                    Gesture::Marquee { a, b, op } => {
                        let tiny = ((a.0 - b.0) * ppf).abs() < 3.0 && (a.1 - b.1).abs() < 3.0;
                        let found = in_area(a, b);
                        match op {
                            SelOp::Replace => self.sel = if tiny { BTreeSet::new() } else { found.into_iter().collect() },
                            SelOp::Add => self.sel.extend(found),
                            SelOp::Remove => {
                                for id in found {
                                    self.sel.remove(&id);
                                }
                            }
                        }
                    }
                }
            }
            self.scrubbing = false;
            if let Some(d) = self.drag.take() {
                let (a, b) = (d.anchor.min(d.current), d.anchor.max(d.current) + 1);
                let row = &m.groups[d.group].rows[d.row];
                let mut spans: Vec<(u64, u64)> = vec![];
                if d.press {
                    spans = row.spans.clone();
                    spans.push((a, b));
                } else {
                    for &(s, e) in &row.spans {
                        if e <= a || s >= b {
                            spans.push((s, e));
                        } else {
                            if s < a {
                                spans.push((s, a));
                            }
                            if e > b {
                                spans.push((b, e));
                            }
                        }
                    }
                }
                action = Some(Action::Edit { edits: vec![(row.key, merge(spans))], from: a });
            }
        }

        // Delete removes the selected presses; Escape clears the selection.
        if self.mode == Mode::Select && !self.sel.is_empty() && !ui.ctx().egui_wants_keyboard_input() {
            let (del, esc) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                )
            });
            if del {
                let gone = sel_spans(&self.sel);
                let keys: BTreeSet<InputKey> = gone.iter().map(|(k, _)| *k).collect();
                let mut edits = vec![];
                for key in keys {
                    if let Some((gi, ri)) = row_of(m, key) {
                        let spans = m.groups[gi].rows[ri].spans.iter().copied().filter(|sp| !gone.contains(&(key, *sp))).collect();
                        edits.push((key, spans));
                    }
                }
                if let Some(from) = gone.iter().map(|(_, sp)| sp.0).min() {
                    action = Some(Action::Edit { edits, from });
                }
                self.sel.clear();
            } else if esc {
                self.sel.clear();
            }
        }
        // What would be selected if the drag ended now, for highlighting.
        let shown_sel: BTreeSet<PressId> = match &self.gesture {
            Some(Gesture::Marquee { a, b, op }) => {
                let found = in_area(*a, *b);
                let mut s = if *op == SelOp::Replace { BTreeSet::new() } else { self.sel.clone() };
                if *op == SelOp::Remove {
                    for id in found {
                        s.remove(&id);
                    }
                } else {
                    s.extend(found);
                }
                s
            }
            _ => self.sel.clone(),
        };
        let move_delta = match &self.gesture {
            Some(Gesture::Move { delta, .. }) => *delta,
            _ => 0,
        };

        // ---- drawing ----
        let bg = Color32::from_gray(22);
        painter.rect_filled(rect, 0.0, bg);
        let step = tick_step(ppf);
        let first_tick = (f0 / step) * step;

        // Rows.
        let rows_clip = Rect::from_min_max(Pos2::new(rect.left(), rows_top), rect.max);
        let rp = painter.with_clip_rect(rows_clip);
        let end_x = x_of(m.frames as f32).min(rect.right());
        for (y0, y1, item) in &items {
            let top = rows_top + y0 - self.scroll_y;
            let bot = rows_top + y1 - self.scroll_y;
            if bot < rows_top || top > rect.bottom() {
                continue;
            }
            let row_rect = Rect::from_min_max(Pos2::new(tl_left, top), Pos2::new(rect.right(), bot));
            match *item {
                Item::Header(gi) => {
                    let g = &m.groups[gi];
                    let c = port_color(g.port, g.device);
                    let tint = if g.used() { c.gamma_multiply(0.18) } else { Color32::from_gray(30) };
                    rp.rect_filled(Rect::from_min_max(Pos2::new(rect.left(), top), Pos2::new(rect.right(), bot)), 0.0, tint);
                    // The union of the group's presses, so a closed group still shows activity.
                    let col_u = if self.is_open(g) { c.gamma_multiply(0.35) } else { c.gamma_multiply(0.8) };
                    for &(a, b) in visible(&g.union, f0, f1) {
                        let x0 = x_of(a as f32);
                        let x1 = x_of(b as f32).max(x0 + 1.5);
                        rp.rect_filled(Rect::from_min_max(Pos2::new(x0, top + 6.0), Pos2::new(x1, bot - 6.0)), 1.0, col_u);
                    }
                    let a = g.other.partition_point(|f| *f < f0);
                    let b = g.other.partition_point(|f| *f < f1);
                    for f in &g.other[a..b] {
                        let x = x_of(*f as f32);
                        rp.line_segment([Pos2::new(x, top + 3.0), Pos2::new(x, bot - 3.0)], Stroke::new(1.0, Color32::from_rgb(235, 200, 120)));
                    }
                    rp.line_segment([Pos2::new(rect.left(), bot - 0.5), Pos2::new(rect.right(), bot - 0.5)], Stroke::new(1.0, Color32::from_gray(45)));
                }
                Item::Row(gi, ri) => {
                    let g = &m.groups[gi];
                    let row = &g.rows[ri];
                    if ri % 2 == 1 {
                        rp.rect_filled(row_rect, 0.0, Color32::from_gray(26));
                    }
                    let c = port_color(g.port, g.device);
                    let span_rect = |a: u64, b: u64| {
                        let x0 = x_of(a as f32);
                        Rect::from_min_max(Pos2::new(x0, top + 2.0), Pos2::new(x_of(b as f32).max(x0 + 1.5), bot - 2.0))
                    };
                    let resizing = match &self.gesture {
                        Some(Gesture::Resize { key, span, left, to }) if *key == row.key => Some((*span, if *left { (*to, span.1) } else { (span.0, *to) })),
                        _ => None,
                    };
                    let hl = Stroke::new(1.5, Color32::WHITE);
                    for &(a, b) in visible(&row.spans, f0, f1) {
                        let selected = shown_sel.contains(&(row.key, a));
                        if let Some((old, new)) = resizing {
                            if old == (a, b) {
                                rp.rect_filled(span_rect(new.0, new.1), 2.0, c);
                                rp.rect_stroke(span_rect(new.0, new.1), 2.0, hl, StrokeKind::Inside);
                                continue;
                            }
                        }
                        if selected && move_delta != 0 {
                            // Where the press was; the moved copy is drawn below.
                            rp.rect_stroke(span_rect(a, b), 2.0, Stroke::new(1.0, c.gamma_multiply(0.6)), StrokeKind::Inside);
                            continue;
                        }
                        rp.rect_filled(span_rect(a, b), 2.0, if selected { c.linear_multiply(1.25) } else { c });
                        if selected {
                            rp.rect_stroke(span_rect(a, b), 2.0, hl, StrokeKind::Inside);
                        }
                    }
                    if move_delta != 0 {
                        for &(a, b) in &row.spans {
                            if self.sel.contains(&(row.key, a)) {
                                let (na, nb) = ((a as i64 + move_delta) as u64, (b as i64 + move_delta) as u64);
                                rp.rect_filled(span_rect(na, nb), 2.0, c);
                                rp.rect_stroke(span_rect(na, nb), 2.0, hl, StrokeKind::Inside);
                            }
                        }
                    }
                    if let Some(d) = &self.drag {
                        if d.group == gi && d.row == ri {
                            let (a, b) = (d.anchor.min(d.current), d.anchor.max(d.current) + 1);
                            let r = Rect::from_min_max(Pos2::new(x_of(a as f32), top + 1.0), Pos2::new(x_of(b as f32).max(x_of(a as f32) + 2.0), bot - 1.0));
                            if d.press {
                                rp.rect_filled(r, 2.0, Color32::WHITE.gamma_multiply(0.7));
                            } else {
                                rp.rect_filled(r, 0.0, bg.gamma_multiply(0.9));
                                rp.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_rgb(230, 90, 80)), StrokeKind::Inside);
                            }
                        }
                    }
                }
            }
        }
        // Grid lines, per-frame lines when zoomed in, and the area past the end.
        let grid = painter.with_clip_rect(Rect::from_min_max(Pos2::new(tl_left, rect.top()), rect.max));
        if ppf >= 6.0 {
            for f in f0..f1 {
                let x = x_of(f as f32);
                grid.line_segment([Pos2::new(x, rows_top), Pos2::new(x, rect.bottom())], Stroke::new(1.0, Color32::from_white_alpha(6)));
            }
        }
        let mut t = first_tick;
        while t <= f1 {
            let x = x_of(t as f32);
            grid.line_segment([Pos2::new(x, rect.top() + RULER_H * 0.5), Pos2::new(x, rect.bottom())], Stroke::new(1.0, Color32::from_white_alpha(14)));
            grid.text(Pos2::new(x + 3.0, rect.top() + 2.0), Align2::LEFT_TOP, t.to_string(), FontId::monospace(10.0), Color32::from_gray(150));
            t += step;
        }
        if end_x < rect.right() {
            grid.rect_filled(Rect::from_min_max(Pos2::new(end_x, rect.top()), rect.max), 0.0, Color32::from_black_alpha(110));
            grid.line_segment([Pos2::new(end_x, rect.top()), Pos2::new(end_x, rect.bottom())], Stroke::new(1.0, Color32::from_gray(90)));
        }
        grid.line_segment([Pos2::new(tl_left, rect.top() + RULER_H), Pos2::new(rect.right(), rect.top() + RULER_H)], Stroke::new(1.0, Color32::from_gray(50)));
        grid.line_segment([Pos2::new(tl_left, rows_top), Pos2::new(rect.right(), rows_top)], Stroke::new(1.0, Color32::from_gray(50)));

        // Markers lane.
        let mut hover_text: Option<String> = None;
        for (bi, (_, x, ixs)) in boxes.iter().enumerate() {
            let first = &m.markers[ixs[0]];
            let y = row_y(box_row[bi]);
            grid.line_segment([Pos2::new(*x, y + MARK_H * 0.5), Pos2::new(*x, rect.bottom())], Stroke::new(1.0, first.color.gamma_multiply(0.35)));
            grid.text(Pos2::new(*x, y), Align2::CENTER_CENTER, first.glyph, FontId::proportional(12.0), first.color);
        }
        for (x, y, text, c) in &marker_labels {
            grid.text(Pos2::new(*x, *y), Align2::LEFT_CENTER, text, font.clone(), *c);
        }
        if let Some(bi) = pointer.and_then(marker_at) {
            let (f, _, ixs) = &boxes[bi];
            let lines: Vec<String> = ixs.iter().map(|i| format!("frame {}: {}", m.markers[*i].frame, m.markers[*i].text)).collect();
            hover_text = Some(format!("{}\nclick to edit frame {f}", lines.join("\n")));
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // Playhead and hover line.
        if let Some(ph) = playhead {
            let x0 = x_of(ph as f32);
            let x1 = x_of(ph as f32 + 1.0);
            let hl = Color32::from_rgb(255, 230, 120);
            if x1 - x0 >= 3.0 {
                grid.rect_filled(Rect::from_min_max(Pos2::new(x0, rect.top()), Pos2::new(x1, rect.bottom())), 0.0, hl.gamma_multiply(0.12));
            }
            let x = (x0 + x1) * 0.5;
            grid.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], Stroke::new(1.5, hl));
            grid.add(egui::Shape::convex_polygon(vec![Pos2::new(x - 5.0, rect.top()), Pos2::new(x + 5.0, rect.top()), Pos2::new(x, rect.top() + 7.0)], hl, Stroke::NONE));
        }
        if let Some(p) = pointer {
            if rect.contains(p) && p.x >= tl_left && self.drag.is_none() {
                let f = frame_at(p.x);
                if f < m.frames {
                    let x = x_of(f as f32 + 0.5);
                    grid.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], Stroke::new(1.0, Color32::from_white_alpha(30)));
                    if hover_text.is_none() && p.y >= rows_top {
                        if let Some(ix) = hit(p) {
                            let what = match items[ix].2 {
                                Item::Header(gi) => m.groups[gi].name.clone(),
                                Item::Row(gi, ri) => format!("{} {}", m.groups[gi].name, m.groups[gi].rows[ri].label),
                            };
                            hover_text = Some(format!("{what} · frame {f}"));
                        }
                    }
                }
            }
        }
        if let Some(d) = &self.drag {
            let (a, b) = (d.anchor.min(d.current), d.anchor.max(d.current));
            hover_text = Some(format!("{} frames {a}..={b}", if d.press { "press" } else { "release" }));
        }
        match &self.gesture {
            Some(Gesture::Marquee { a, b, op }) => {
                let to_screen = |(f, y): (f32, f32)| Pos2::new(x_of(f), rows_top + y - self.scroll_y);
                let r = Rect::from_two_pos(to_screen(*a), to_screen(*b));
                let mc = if *op == SelOp::Remove { Color32::from_rgb(230, 90, 80) } else { Color32::from_rgb(120, 180, 255) };
                let mp = painter.with_clip_rect(Rect::from_min_max(Pos2::new(tl_left, rows_top), rect.max));
                mp.rect_filled(r, 0.0, mc.gamma_multiply(0.12));
                mp.rect_stroke(r, 0.0, Stroke::new(1.0, mc), StrokeKind::Inside);
            }
            Some(Gesture::Move { delta, .. }) if *delta != 0 => hover_text = Some(format!("move {} press{} by {delta:+} frames", self.sel.len(), if self.sel.len() == 1 { "" } else { "es" })),
            Some(Gesture::Resize { span, left, to, .. }) => {
                let (a, b) = if *left { (*to, span.1) } else { (span.0, *to) };
                hover_text = Some(format!("press frames {a}..={} ({} frames)", b - 1, b - a));
            }
            _ => {}
        }

        // Label column, drawn last so it covers the timeline.
        let lp = painter.with_clip_rect(Rect::from_min_max(rect.min, Pos2::new(tl_left, rect.bottom())));
        lp.rect_filled(Rect::from_min_max(rect.min, Pos2::new(tl_left, rect.bottom())), 0.0, Color32::from_gray(28));
        lp.text(Pos2::new(rect.left() + 6.0, rect.top() + RULER_H * 0.5), Align2::LEFT_CENTER, "frame", FontId::proportional(11.0), Color32::from_gray(120));
        lp.text(Pos2::new(rect.left() + 6.0, my), Align2::LEFT_CENTER, "markers", FontId::proportional(11.0), Color32::from_gray(120));
        let lrows = lp.with_clip_rect(Rect::from_min_max(Pos2::new(rect.left(), rows_top), Pos2::new(tl_left, rect.bottom())));
        for (y0, y1, item) in &items {
            let top = rows_top + y0 - self.scroll_y;
            let bot = rows_top + y1 - self.scroll_y;
            if bot < rows_top || top > rect.bottom() {
                continue;
            }
            let mid = (top + bot) * 0.5;
            match *item {
                Item::Header(gi) => {
                    let g = &m.groups[gi];
                    let c = port_color(g.port, g.device);
                    let open = self.is_open(g);
                    if g.used() {
                        lrows.rect_filled(Rect::from_min_max(Pos2::new(rect.left(), top), Pos2::new(rect.left() + 4.0, bot)), 0.0, c);
                        lrows.rect_filled(Rect::from_min_max(Pos2::new(rect.left() + 4.0, top), Pos2::new(tl_left, bot)), 0.0, c.gamma_multiply(0.22));
                    }
                    let tc = if g.used() { Color32::from_gray(235) } else { Color32::from_gray(120) };
                    lrows.text(Pos2::new(rect.left() + 10.0, mid), Align2::LEFT_CENTER, format!("{} {}", if open { "⏷" } else { "⏵" }, g.name), FontId::proportional(12.5), tc);
                    let note = if g.presses > 0 { format!("{}", g.presses) } else if g.used() { "·".into() } else { "—".into() };
                    lrows.text(Pos2::new(tl_left - 6.0, mid), Align2::RIGHT_CENTER, note, FontId::monospace(10.5), if g.used() { c } else { Color32::from_gray(90) });
                }
                Item::Row(gi, ri) => {
                    let g = &m.groups[gi];
                    let row = &g.rows[ri];
                    let has = !row.spans.is_empty();
                    lrows.text(Pos2::new(rect.left() + 26.0, mid), Align2::LEFT_CENTER, &row.label, FontId::monospace(10.5), if has { Color32::from_gray(225) } else { Color32::from_gray(110) });
                    if has {
                        lrows.circle_filled(Pos2::new(rect.left() + 17.0, mid), 2.5, port_color(g.port, g.device));
                    }
                }
            }
        }
        lp.line_segment([Pos2::new(tl_left - 0.5, rect.top()), Pos2::new(tl_left - 0.5, rect.bottom())], Stroke::new(1.0, Color32::from_gray(55)));
        // Vertical scroll indicator.
        if content_h > view_h {
            let frac = view_h / content_h;
            let h = (view_h * frac).max(16.0);
            let y = rows_top + (self.scroll_y / (content_h - view_h)) * (view_h - h);
            painter.rect_filled(Rect::from_min_size(Pos2::new(tl_left - 4.0, y), egui::vec2(3.0, h)), 1.5, Color32::from_gray(90));
        }

        if let Some(t) = hover_text {
            resp.clone().on_hover_ui_at_pointer(|ui| {
                ui.label(t);
            });
        }
        if self.mode == Mode::Select {
            let icon = match &self.gesture {
                Some(Gesture::Move { .. }) => Some(egui::CursorIcon::Grabbing),
                Some(Gesture::Resize { .. }) => Some(egui::CursorIcon::ResizeHorizontal),
                Some(Gesture::Marquee { .. }) => None,
                None if resp.hovered() => pointer.and_then(press_at).map(|(_, _, edge)| if edge.is_some() { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::Grab }),
                None => None,
            };
            if let Some(icon) = icon {
                ui.ctx().set_cursor_icon(icon);
            }
        } else if resp.hovered() && pointer.is_some_and(|p| p.y >= rows_top && p.x >= tl_left) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        action
    }
}
