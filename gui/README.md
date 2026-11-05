# emucov-gui

A desktop viewer and editor for replay collections, built with egui. It loads one
libretro core and lets you browse the replay tree, play and seek with live video,
and edit button inputs on a piano roll. See [QUICKSTART.md](../QUICKSTART.md#gui-prototype)
for how to build, run and use it.

| File | What it does |
| --- | --- |
| `src/main.rs` | The app: menus, replay tree, preview, playback and seeking, applying edits |
| `src/roll.rs` | The piano roll: building rows from a replay, drawing, scrolling, zoom, drag to edit |

The emulation itself goes through the `emucov` library in [player/](../player/)
(`Session::enter_replay`, `restore_inputs`, `step`, `snapshot`), so the GUI and the
CLI play replays the same way. Every recorded Thwaite end hash matches when it is
played in the GUI.

## How seeking works now

- Video frames are not cached. While a replay plays, the GUI keeps a save state
  at the start of every 10th frame (`SNAP_EVERY` in `src/main.rs`).
- A seek loads the nearest save state at or before the target. It sets the held
  inputs again, because input state is not part of a save state. Then it runs
  the core forward at most 9 frames and draws only the last one.
- Save states exist only for frames that playback or scrubbing has already run
  through. The first jump far ahead runs every frame up to that point.
- Long work runs in slices of about 25 ms per screen refresh (`JOB_SLICE`), so
  the window keeps drawing. This covers a seek more than 30 frames past the last
  run frame, and selecting a node whose ancestors have no usable cached state (the
  GUI plays the chain from the nearest clean cache or from power-on). While it
  runs, a progress bar shows in a fixed slot at the right of the status bar, for
  example "Seeking to frame 2354" or "Playing up to "Defend"". The slot is
  reserved when empty, so nothing moves. Clicking another node cancels the job;
  a seek during a node load is applied once the load finishes.
- An edit drops the save states after the edited frame and redraws the current frame.
- Nothing is evicted. Memory grows with replay length (an NES state is a few
  tens of KB, about 10 MB for a 3600-frame replay). That is fine for NES, but not
  for cores with large states (N64, PS1, and so on).

## Planned: a configurable save-state cache (not implemented yet)

1. **The connector declares its state size.** Each core's connector config gets
   a field for the approximate size of one save state, for example `state_size =
   "30KiB"`. This tells the GUI how many states fit in a given amount of memory
   without measuring first. If the field is missing, the GUI can measure the
   first `retro_serialize_size`.
2. **The user sets a maximum cache size.** This is a GUI setting (and could also
   be a per-collection local setting), for example `max_state_cache = "256MiB"`.
   The number of states it can keep is the cache size divided by the state size.
3. **Keep a tail of states during playback.** Save states go into a ring buffer
   that follows the playhead. When the budget is full, the oldest states furthest
   from the playhead are dropped first.
   - The spacing between states can be derived from the budget. If the budget
     covers the whole replay at one state per frame, keep every frame. Otherwise
     space them out so the tail covers a useful window, such as the last N seconds.
   - The state at frame 0 of the replay is always kept, and so are sparse anchors
     (for example every few hundred frames). A long seek backwards then never
     needs to replay from the parent.
4. **Optional video cache.** For slow cores, keep the most recent decoded frames
   (also within the budget) so that scrubbing backwards inside the tail does not
   need the core at all.

Open question: where the connector config lives. Today a collection's
`collection.toml` has a `[core]` section (name, build, options), and
`connectors/<core>/` has only a patch and build script. A small
`connectors/<core>/connector.toml` that the build script copies next to the
built core could hold `state_size` and other per-core facts, such as the
frame rate or which memory regions are worth hashing.

## Open question: splits and coverage

Splitting a node changes no state: the same frames still run with the same inputs.
Coverage, however, will be recorded per node once the GUI tracks it. After a split, the
coverage stored for the old node (now the second half, under the same id) covers both
halves. The recorded coverage for both parts needs to be marked stale and gathered again,
or the old record needs to be cut at the split frame. The cut is only possible if coverage
is stored per frame range, so this decision belongs with the coverage tracker design.

## Other known gaps

- Only button inputs get rows. Analog, lightgun and pointer inputs show as ticks
  on their controller's header.
- You can't play the game from the keyboard yet (the Input menu has a placeholder).
- Only one core runs at a time. Showing two cores side by side will need a second
  process, because a libretro core has global state.
- There is no warning before you lose unsaved edits (on Open, Reload or Quit).
- Nodes can't be deleted, moved or merged from the GUI yet.
- A long load cannot be cancelled except by selecting another node. MAX speed still runs on the UI thread.
- Tested on Linux and macOS (build only); Windows has not been tried.
