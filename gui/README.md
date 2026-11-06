# dj-gui

A desktop viewer and editor for replay collections, built with egui. It loads one
libretro core and lets you browse the replay tree, play and seek with live video,
and edit button inputs on a piano roll. See [QUICKSTART.md](../QUICKSTART.md#gui-prototype)
for how to build, run and use it.

| File | What it does |
| --- | --- |
| `src/main.rs` | The app: menus, replay tree, preview, playback and seeking, applying edits |
| `src/roll.rs` | The piano roll: building rows from a replay, drawing, scrolling, zoom, drag to edit |

The emulation itself goes through the `decomp_dj` library in [player/](../player/)
(`Session::enter_replay`, `restore_inputs`, `step`, `snapshot`), so the GUI and the
CLI play replays the same way. Every recorded Thwaite end hash matches when it is
played in the GUI.

## How seeking works now

- Video frames are not cached. While a replay plays, the GUI keeps a save state
  in memory at the start of every 10th frame (`SNAP_EVERY` in `src/main.rs`).
- A seek uses the nearest snapshot at or before the target: from memory, or from
  the local database (`.local/cache.db`) when a stored one is nearer. It sets
  the held inputs again, because input state is not part of a save state, then
  runs the core forward and draws only the last frame. How many snapshots the
  database has depends on the collection's `[cache]` settings (QUICKSTART.md);
  with `diff_every = 60`, a seek anywhere is near instant.
- Selecting a segment starts from its parent's end snapshot in the database when the
  parent is clean, so deep segments open without replaying the chain. The GUI also
  stores each segment's start snapshot, and the end snapshot of segments with children.
- Long work runs in slices of about 25 ms per screen refresh (`JOB_SLICE`), so
  the window keeps drawing. This covers a seek more than 30 frames past the last
  run frame, and selecting a segment whose ancestors have no stored state (the GUI
  plays the chain). While it runs, a progress bar shows in a fixed slot at the
  right of the status bar, for example "Seeking to frame 2354" or "Playing up to
  "Defend"". Clicking another segment cancels the job; a seek during a segment load is
  applied once the load finishes.
- An edit drops the save states after the edited frame and redraws the current frame.
- **Memory limit.** In-memory snapshots stay under Emulation > Snapshot memory
  (`max_memory_mb` in `.local/local.toml`, 512 MB by default). Past it, the
  snapshots farthest from the playhead go first; frame 0 always stays, and
  anchors every 300 frames stay while they fit. An NES state is a few tens of KB,
  so NES never reaches it; it matters for cores with large states.

## Hydrating

Emulation > Hydrate all (or right-click > Hydrate this segment / this and below)
runs the shared `decomp_dj::hydrate::Hydrator` in the same 25 ms slices, showing
every 4th frame. Selecting, seeking and playing wait until it ends; Pause and
Stop are in the status bar. Afterwards the selected segment is loaded again.

## Open question: splits and coverage

Splitting a segment changes no state: the same frames still run with the same inputs.
Coverage, however, will be recorded per segment once the GUI tracks it. After a split, the
coverage stored for the old segment (now the second half, under the same id) covers both
halves. The recorded coverage for both parts needs to be marked stale and gathered again,
or the old record needs to be cut at the split frame. The cut is only possible if coverage
is stored per frame range, so this decision belongs with the coverage tracker design.

## Undo and redo

Every edit records the affected segments' full text before the change as one undo step
(`remember` in `src/main.rs`): note edits, cut and paste, frame lines, renames, segment
properties, accepting an end hash, new segments, splits and branches. Undo puts those segments
back (removing segments the step created) and records the reverse step for redo. The history
holds 200 steps and is cleared when a collection is opened or reloaded.

After an undo on the segment being shown, playback drops its save states from the first
changed frame on and redraws where you are. If the step changed the segment's length or
parent, another segment, or one of its ancestors, the segment is loaded again instead. A segment
removed by undo has its file deleted on the next Save.

Cut, copy and paste work on selected notes in the piano roll. The copy is kept inside the
app, and the notes also go on the system clipboard as replay lines with frames relative
to the first note. Paste puts the leftmost copied note at the play cursor.

## Deleting segments

Segment > Delete… and Delete… in the tree's right-click menu open the same confirmation.
It names the segment, warns how many segments below it go too, and gives the total frames.
Deleting removes the whole branch (`delete_nodes` in `src/main.rs`) as one undo step.
The files are removed on Save. If the segment being shown was deleted, its parent is shown.

Later: merge a segment into its children (its inputs go at the start of each child, so no
hash changes; with one child, this is the inverse of Split). Another option is moving
the children up to the grandparent, which turns everything below dirty. Neither is planned yet.

## Marker list (implemented)

The left sidebar has two tabs, **Segments** (the tree) and **Markers**. Markers lists every
event in the collection alphabetically, then RAM writes, resets, device changes and RNG
lines. Each row shows the frame and the segment (with its parent when another segment has the
same title), and the filter matches marker text and segment titles. Clicking a row selects
the segment and seeks to the frame. This lets markers do the job of tiny segments that only
exist to name a moment. Checkpoints and screenshots aren't listed, since they have no
names. Ideas: group repeated texts, follow the tree's Focus here, and list a marker's
"first frame where a watched value differs" once that exists.

## Joining segments (implemented: join with parent)

Joining a segment to its parent keeps every frame and input in the same order, so the
emulator does exactly the same thing and every hash below stays valid.

**Join with parent** (Segment menu and right-click, on a segment whose parent has only that
child) asks first. The parent keeps its id and title and grows by the child's frames. The
child's lines move by the parent's length (inputs the parent still held are released at
the join, as a segment start would), the child's children move up, and the end hash comes
from the child. The child's start hash becomes a checkpoint at the join frame. The dialog
has "Leave a marker", on by default, with an editable text that starts as the child's
title: an event at the join frame keeps whatever moment made the author split there
("menu fully open", "victory pose finished"). Undo splits them again.

Not done yet:

2. **Join with only child** (same operation, offered on the parent).
3. **Merge into children** (the parent has several children): copy the parent's frames
   onto the front of every child, then remove the parent. No hash changes, but the frames
   are duplicated in each child.
4. **Join a chain**: select a run of only-children in the tree and join them all at once.

## Other known gaps

- Only button inputs get rows. Analog, lightgun and pointer inputs show as ticks
  on their controller's header.
- You can't play the game from the keyboard yet (the Input menu has a placeholder).
- Only one core runs at a time. Showing two cores side by side will need a second
  process, because a libretro core has global state.
- There is no warning before you lose unsaved edits (on Open, Reload or Quit).
- Segments can't be moved from the GUI yet.
- A long load cannot be cancelled except by selecting another segment. MAX speed still runs on the UI thread.
- Tested on Linux and macOS (build only); Windows has not been tried.
