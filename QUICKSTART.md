# Quickstart

What is here so far (first plan, steps 1 to 4, plus a GUI prototype):

| Path | What it is |
| --- | --- |
| [SPEC.md](SPEC.md) | The replay collection format, v1 draft |
| [player/](player/) | `dj`, the headless player (Rust): plays replay trees on any libretro core, checks state hashes, records and reports coverage |
| [gui/](gui/) | `dj-gui`, a desktop viewer and editor (Rust, egui): replay tree, live preview, piano roll |
| [connectors/](connectors/) | Per-core coverage patches. [mesen](connectors/mesen/) (NES) is the first |
| [examples/thwaite/](examples/thwaite/) | A 12-replay collection for a free NES game, used as the end-to-end test |

## Build

```sh
cd player && cargo build --release        # Rust 1.75+; the binary is player/target/release/dj
cd gui && cargo build --release           # Rust 1.95+ (egui 0.37); the binary is gui/target/release/dj-gui
connectors/mesen/build.sh                 # git, make, a C++11 compiler; output in build/cores/
```

## Commands

```sh
dj init  <dir> --name NAME --rom ROM --core CORE [--option KEY=VALUE]... [--symbols FILE]
dj new   <dir> [--parent ID] [--title TEXT] [--frames N]     # prints the new replay's path
dj status <dir>                                              # tree with clean / dirty / unverified marks
dj play  <dir> <id> [--screenshots DIR] [--shot 60,120] [--no-cache] [--accept]
dj run   <dir> [--from-scratch] [--accept] [--screenshots]   # play everything, check hashes, record coverage
dj hydrate <dir> [SEGMENT [--children]] [--clean] [--force]  # fill the local database
dj report <dir> [--json FILE] [--gaps FILE.csv]              # coverage of the current, clean segments
dj where <dir> PRG:0x1234 | 0x8123 | SYMBOL                  # which segments first run it, at which frame
dj snapshot <dir> SEGMENT FRAME --out FILE                   # the core state there, to start a trace from
dj sql   <dir> "SELECT ..."                                  # query the local database
dj cache stats <dir> | dj cache gc <dir>                     # size, or drop what no segment uses
```

Every command that plays takes `--no-db` to run without the local database.

- `--rom` and `--core` are remembered per collection in `.local/local.toml`, which is never committed.
- Ids can be shortened to any unique prefix.
- `init` records **every** core option's current value in `collection.toml`, so the setup is pinned.
- `run` exits with an error when a replay's hashes differ from the recorded ones. `--accept` writes the new hashes instead.
- `run --from-scratch` replays every chain from power-on instead of from cached parent states, which checks that save states round-trip exactly.

### The local database

`.local/cache.db` (SQLite, never committed) holds what playing produced, keyed by each segment's content (its setup, start state and lines), so an edit invalidates only that segment. The design is in [CACHE_PLAN.md](CACHE_PLAN.md).

- **Snapshots** of the core state at frame 0, every `diff_every` frames, at markers, and at the end of segments with children. Each is split into pages that are stored once by hash and compressed with zstd.
- **Previews** (PNG) at the start and at markers, coverage flags, and the frame each ROM byte first ran in each segment (for `where`).
- `hydrate` plays only the segments whose content isn't stored yet; running it again is nearly instant. `SEGMENT --children` limits it to a subtree. `--clean` re-checks dirty segments: one that still ends with its recorded end hash and checkpoints gets its new start hash written to its file (it is clean again); the others are listed as mismatches. `--force` plays even what is stored.
- `report` reads the database, not the files: it counts only clean segments that have been hydrated, and lists how many were dirty or not played yet. Address ranges in `unreachable.txt` at the collection root (`0x8000-0x80FF # comment`, optionally `PRG:` first) are left out of the totals.
- `cache gc` deletes contents that no current segment uses, then unused snapshots and pages, and compacts the file.
- The local folder used to be called `.emucov/`. The tools move it to `.local/` the first time they open a collection (and change `.emucov/` to `.local/` in the collection's `.gitignore`). Its old `coverage.sqlite` and `states/` from before the database are no longer used and can be deleted.

Settings go in `collection.toml` (shared) or `.local/local.toml` (this machine, wins):

```toml
[cache]
preset = "nes-sparse"   # nes-sparse (default), nes-aggressive, n64-sparse, n64-aggressive
page_kb = 4             # split snapshots into pages of this size; missing, or the snapshot size or more, keeps them whole
diff_every = 300        # also store a snapshot every N frames (0: only at frame 0 and markers)
diff_at_markers = true
previews = "thumb"      # none, thumb (half size) or full
```

`.local/local.toml` also takes `database = false` (play without it, like `--no-db`) and `max_memory_mb` (the GUI's in-memory snapshot limit).

## GUI (prototype)

```sh
cd gui && cargo build --release
target/release/dj-gui ../examples/thwaite       # or File > Open collection…
```

The ROM and core come from the collection's `.local/local.toml` (written by the CLI's `--rom`/`--core`), or set them with Emulation > Set ROM… / Set core….

- Left: two tabs, **Segments** and **Markers**. Markers lists every event in the collection alphabetically (then writes and other lines) with the frame and segment; its filter matches marker text and segment titles, and clicking a row goes to that segment and frame.
- Segments: the replay tree. Dots show clean (green), dirty (orange) and unverified (gray); anything under a dirty replay is dirty too. Edited replays are yellow italics until saved. Click a segment to go to it; double-click to play it; Enter (or F2) renames the selected segment in place (Enter or clicking away keeps the new name, Escape cancels). The filter box at the top matches titles, id prefixes and tags, and shows each match with the path down to it (the path is dimmed).
  - Indentation only steps in where a segment has more than one child. An only child stays in its parent's column, joined to it by a short line, so a long chain of segments stays one column however deep it goes.
  - The third icon (a stepped tree) switches to indenting every level, which reads better for small trees; click it again to keep only children in line. The two icons before it collapse or expand every segment. Collapse all keeps the focused segment open, so its children still show.
  - **Sticky segments**: the tree remembers the deepest segment you've been in. Going back up its chain highlights the chain above the selection, and the segments below it that lead back to the deepest one are highlighted faintly with faded titles. Continue and the child button follow those even where a segment has several children. Selecting a segment off that chain starts a new one.
  - Right-click > **Focus here** shows only that segment and what is below it. A breadcrumb bar above the filter shows the path to it (long paths fold the middle into a "… N" menu); click any crumb to focus there, or **All** for the whole tree. The filter searches inside the focused part only.
- Top: the emulator picture, with the transport below it, left to right:
  - **⏮** (start) goes to frame 0; pressed again at frame 0, it goes back to the last frame of the parent segment (off at the start of a root segment). **◀●** goes to the previous marker in this segment.
  - Step back, play/pause (Space), step forward. The left and right arrow keys step too; Home and End go to the first and last frame.
  - **●▶** goes to the next marker in this segment. Both marker buttons are off when there is no marker that way.
  - **⏭** (end) goes to the last frame; pressed again at the end, it goes on into a child segment (carrying straight on from the current state): the sticky segment (where you were), or the only child, or, with several, it opens the list. It is a split button: the ▾ beside it lists every child at any time (faint when there is only one). At the end of a segment with no children it is off.
  - **↪** (continue, on by default) carries playback on into the next segment at the end of a segment: the only child, or the sticky segment, as long as neither has unsaved changes.
  - Speed: a dropdown of 0.25×, 0.5×, 1×, 2×, 4×, 8× and MAX, with − and + beside it. Reaching the last frame checks the end hash.
- Bottom: the piano roll. Time runs left to right; one row per button, grouped by controller. Groups the replay uses are tinted and open, unused ones dimmed and closed; click a group name to open or close it. A closed group still shows when any of its buttons are held.
  - Wheel or trackpad over the timeline scrolls time; Ctrl/Cmd+wheel zooms at the pointer; wheel over the names scrolls rows; "fit" shows the whole replay.
  - Click or drag on the ruler to seek.
  - One editing mode (hover the ⓘ in the roll's toolbar for a summary):
    - Click an empty cell to add a press as long as the last press made or resized (4 frames to begin with).
    - Drag a press to move it in time, and drag either edge to lengthen or shorten it (only while at most one press is selected; the grab zone is a thin strip on the edge). Plain drags never change rows.
    - Transpose (move presses to another button row of the same controller) only on purpose: hold Alt (Option on a Mac) while dragging, or press Alt+Up / Alt+Down with presses selected.
    - Click selects a press; Shift+click adds or removes it. Drag on empty space to select an area (with Shift it adds, with Shift+Alt it removes).
    - Right-click a press, or press Delete, to remove it. Escape clears the selection. Presses that end up overlapping merge into one. Holding a drag near either end of the roll scrolls it.
  - The marker lane shows events, RAM writes, [DEVICE]/[RESET] lines, screenshots and checkpoints. Click "markers" in the label column to open or close it, like a controller. Closed, it is one row: markers on one frame share a box, and markers too close together hide behind the most important one (events first), which shows "+N" for the rest. Open, every marker gets its own box with its whole label: events in the top rows, writes and everything else below. Labels run until the next marker in their row. Hover for all markers at that spot; click one to edit, delete or add lines on that frame. **E** adds an event at the playhead.
- Edit menu: **Undo** / **Redo** (Ctrl/Cmd+Z, Ctrl/Cmd+Shift+Z or Ctrl+Y) cover every edit, including splits and new segments. **Cut**, **Copy** and **Paste** (Ctrl/Cmd+X/C/V) work on the notes selected in the piano roll; paste puts the leftmost copied note at the play cursor and selects what it pasted.
- Segment menu (and right-click in the tree): **Play to here** (right-click only: plays through to the end of that segment, on from the playhead if the current segment is on the way, otherwise from the first segment below where the two paths meet); **New child at end**; **Branch at playhead** (splits the segment right after the playhead and starts an empty second path beside the rest); **Split at playhead** (just the split); **Properties** (title and length). **Join with parent** (only for a segment with no siblings) merges it into its parent, which keeps its name and grows by its frames; the confirmation can leave a marker at the join, named after the child by default. **Delete…** removes the segment and everything below it, after a confirmation that says how many segments go; undo brings them back, and the files are removed on Save.
  - In a split, the second half keeps the segment's id, title and children; the first part becomes a new segment "(first N frames)" in its old place, and the second half becomes its child. The state hash at the cut becomes the first part's end hash and the second half's start hash; the second half keeps the old end hash and everything below it is assumed clean. Inputs held across the cut are pressed again at frame 0 of the second half.
- Bottom edge: the status bar, with messages on the left and the core and unsaved count on the right. A long seek or segment load shows a progress bar there and the window stays usable.
- The local database (Emulation menu): **Hydrate all**, **Hydrate and clean all** (only with nothing unsaved, since cleaning writes files), and right-click **Hydrate this segment** / **Hydrate this and below**. Hydration shows every 4th frame in the picture and a progress bar with **Pause** and **Stop** in the status bar; finished segments stay stored when you stop. Segments whose current content is stored show a blue **db** after their length. **Use the local database** turns it off (saved in `.local/local.toml`), and **Snapshot memory** sets how much memory the current segment's snapshots may use (512 MB by default).
- An input or RAM write edit drops the segment's end hash and later checkpoints, so it and everything below turn unverified or dirty. Event edits don't. Play to the end and use Emulation > Accept end hash to record the new hashes, then File > Save (Ctrl/Cmd+S).

## Results on the Thwaite example

- All 12 replays reproduce their recorded hashes on Linux x86_64 (where they were recorded) and on macOS arm64 (Apple clang 17), both from cached parent states and from scratch. The ROM builds byte-identical on both.
- Coverage from 11,140 frames: 114 of 134 functions entered, 835 of 1,087 basic blocks, 3,733 of 4,656 instructions (80.2%).
- The 5-replay smallest set keeps all of it.
- Playback from cached parent states runs at about 480 frames per second on one Linux x86_64 cloud core and 717 on an Apple silicon MacBook Air (8 to 12 times real time). The coverage patch costs about 5% against stock Mesen, with identical hashes.
- The gaps are what you would expect from short, unskilled play: balloons and MIRVs, end-of-round scoring, and rebuilding houses and silos. Each needs a replay that survives a full round.

## Known limits

- Only one core has a connector (Mesen, NES).
- Block and function rollup is 6502-only. Other CPUs get raw executed-byte counts until their disassembler is added.
- Determinism has been checked on Linux x86_64 and macOS arm64; Windows still needs a run.
- The GUI is a first prototype: button inputs only (no analog, lightgun or pointer rows yet), no keyboard play/recording, one core at a time. Tested on Linux; macOS and Windows not tried yet.
