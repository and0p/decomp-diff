# Quickstart

What is here so far (first plan, steps 1 to 4, plus a GUI prototype):

| Path | What it is |
| --- | --- |
| [SPEC.md](SPEC.md) | The replay collection format, v1 draft |
| [player/](player/) | `emucov`, the headless player (Rust): plays replay trees on any libretro core, checks state hashes, records and reports coverage |
| [gui/](gui/) | `emucov-gui`, a desktop viewer and editor (Rust, egui): replay tree, live preview, piano roll |
| [connectors/](connectors/) | Per-core coverage patches. [mesen](connectors/mesen/) (NES) is the first |
| [examples/thwaite/](examples/thwaite/) | A 12-replay collection for a free NES game, used as the end-to-end test |

## Build

```sh
cd player && cargo build --release        # Rust 1.75+; the binary is player/target/release/emucov
cd gui && cargo build --release           # Rust 1.95+ (egui 0.37); the binary is gui/target/release/emucov-gui
connectors/mesen/build.sh                 # git, make, a C++11 compiler; output in build/cores/
```

## Commands

```sh
emucov init  <dir> --name NAME --rom ROM --core CORE [--option KEY=VALUE]... [--symbols FILE]
emucov new   <dir> [--parent ID] [--title TEXT] [--frames N]     # prints the new replay's path
emucov status <dir>                                              # tree with clean / dirty / unverified marks
emucov play  <dir> <id> [--screenshots DIR] [--shot 60,120] [--no-cache] [--accept]
emucov run   <dir> [--from-scratch] [--accept] [--screenshots]   # play everything, check hashes, record coverage
emucov report <dir> [--json FILE] [--gaps FILE.csv]
```

- `--rom` and `--core` are remembered per collection in `.emucov/local.toml`, which is never committed.
- Ids can be shortened to any unique prefix.
- `init` records **every** core option's current value in `collection.toml`, so the setup is pinned.
- `run` exits with an error when a replay's hashes differ from the recorded ones. `--accept` writes the new hashes instead.
- `run --from-scratch` replays every chain from power-on instead of from cached parent states, which checks that save states round-trip exactly.

## GUI (prototype)

```sh
cd gui && cargo build --release
target/release/emucov-gui ../examples/thwaite       # or File > Open collection…
```

The ROM and core come from the collection's `.emucov/local.toml` (written by the CLI's `--rom`/`--core`), or set them with Emulation > Set ROM… / Set core….

- Left: the replay tree. Dots show clean (green), dirty (orange) and unverified (gray); anything under a dirty replay is dirty too. Edited replays are yellow italics until saved. Click a node to go to it; double-click to play it; Enter (or F2) renames the selected node in place (Enter or clicking away keeps the new name, Escape cancels). The filter box at the top matches titles, id prefixes and tags, and shows each match with the path down to it (the path is dimmed).
  - Indentation only steps in where a node has more than one child. An only child stays in its parent's column, joined to it by a short line, so a long chain of nodes stays one column however deep it goes.
  - The two icons right of the filter box collapse or expand every node. Collapse all keeps the focused node open, so its children still show.
  - Right-click > **Focus here** shows only that node and what is below it. A breadcrumb bar above the filter shows the path to it (long paths fold the middle into a "… N" menu); click any crumb to focus there, or **All** for the whole tree. The filter searches inside the focused part only.
- Top: the emulator picture and transport: start, step back, play/pause (Space), step forward, end (Home/End), speed 1×/2×/4×/8×/MAX, and **continue**, which plays straight on into the next node when a node has exactly one child and neither has unsaved changes. Reaching the last frame checks the end hash.
- Bottom: the piano roll. Time runs left to right; one row per button, grouped by controller. Groups the replay uses are tinted and open, unused ones dimmed and closed; click a group name to open or close it. A closed group still shows when any of its buttons are held.
  - Wheel or trackpad over the timeline scrolls time; Ctrl/Cmd+wheel zooms at the pointer; wheel over the names scrolls rows; "fit" shows the whole replay.
  - Click or drag on the ruler to seek.
  - **Draw** mode (D): drag in a button row to press it for those frames; drag over a held span to release it.
  - **Select** mode (S): click a press to select it; Shift+click adds, Alt+Shift+click removes. Drag on empty space to select an area, with the same modifiers. Drag selected presses sideways to move them, and drag either edge of a press to lengthen or shorten it (only while at most one press is selected; the grab zone is a thin strip on the edge). Presses that end up overlapping merge into one. Holding a drag near either end of the roll scrolls it. Delete removes the selected presses; Escape clears the selection.
  - The marker lane shows events (with their text, up to 40 characters or until the next marker), RAM writes, [DEVICE]/[RESET] lines and checkpoints. Several lines on one frame show as one marker with a count, e.g. "(2) Boss appears". "marker rows" (− / +) adds lane rows so close labels stack instead of being cut short. Hover for all of them; click one to edit, delete or add lines on that frame. **E** adds an event at the playhead.
- Edit menu: **Undo** / **Redo** (Ctrl/Cmd+Z, Ctrl/Cmd+Shift+Z or Ctrl+Y) cover every edit, including splits and new nodes. **Cut**, **Copy** and **Paste** (Ctrl/Cmd+X/C/V) work on the notes selected in the piano roll; paste puts the leftmost copied note at the play cursor and selects what it pasted.
- Node menu (and right-click in the tree): **Play to here** (right-click only: plays through to the end of that node, on from the playhead if the current node is on the way, otherwise from the first node below where the two paths meet); **New child at end**; **Branch at playhead** (splits the node right after the playhead and starts an empty second path beside the rest); **Split at playhead** (just the split); **Properties** (title and length). **Delete…** removes the node and everything below it, after a confirmation that says how many nodes go; undo brings them back, and the files are removed on Save.
  - In a split, the second half keeps the node's id, title and children; the first part becomes a new node "(first N frames)" in its old place, and the second half becomes its child. The state hash at the cut becomes the first part's end hash and the second half's start hash; the second half keeps the old end hash and everything below it is assumed clean. Inputs held across the cut are pressed again at frame 0 of the second half.
- Bottom edge: the status bar, with messages on the left and the core and unsaved count on the right. A long seek or node load shows a progress bar there and the window stays usable.
- An input or RAM write edit drops the node's end hash and later checkpoints, so it and everything below turn unverified or dirty. Event edits don't. Play to the end and use Emulation > Accept end hash to record the new hashes, then File > Save (Ctrl/Cmd+S).

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
