# Roadmap

Ideas and open questions that are not part of format version 1, and the current limits.
When one ships, its user-facing description moves into `docs/` and the entry here goes.

## Known limits

- Only one core has a connector (Mesen, NES).
- Block and function rollup is 6502-only. Other CPUs get raw executed-byte counts until their disassembler is added.
- Determinism has been checked on Linux x86_64 and macOS arm64; Windows still needs a run.
- The GUI is a first prototype: button inputs only (no analog, lightgun or pointer rows yet), no keyboard play or recording, one core at a time. Tested on Linux and macOS; Windows not tried yet.
- `exclude` in `[cache]` is recorded but not applied to snapshots yet.

## GUI
- **Rewinding up the chain.** Stepping or seeking back past frame 0 goes into the
  parent's last frames (loading a snapshot of the parent, from memory or the local
  database), and playing forward from there follows the sticky segments back down. A
  possible next step is one timeline across the whole chain, with the segment
  boundaries drawn on the ruler.
- **Drag to draw in the piano roll.** Today a click makes a press as long as the
  last one made or resized, and a drag on empty space selects. Holding the button down
  on an empty cell and dragging could set the new press's length instead, with
  selecting moved to a modifier or to drags that start outside the rows.
- **Record while playing the list.** The GUI's List mode (the end-of-segment dropdown:
  Stop, Loop, Next, List) plays every segment in tree order, the order hydration uses,
  and at MAX speed it is hydration you can watch. A later step could make it record as
  it plays (snapshots, coverage and first hits for each segment it finishes cleanly), so
  watching the coverage map light up and filling the database are the same run.
- **Audio playback (GUI).** Play the core's audio while a segment plays, with a volume
  slider and a mute button in the playback toolbar. When more than one core is running
  (side-by-side comparison), an A|B toggle picks which core you hear.
- **More ways to join.** Join with only child (the same operation, offered on the parent);
  merge into children (copy the parent's frames onto the front of every child, then remove
  the parent: no hash changes, but the frames are duplicated); and join a chain (select a run
  of only-children and join them all at once).
- **Instead of deleting,** merge a segment into its children, or move its children up to the
  grandparent, which turns everything below dirty.
- **Moving segments** to another parent from the GUI.
- **Markers tab:** group repeated texts, follow the tree's Focus here, and list a marker's
  "first frame where a watched value differs" once that exists.
- **A warning before losing unsaved edits** on Open, Reload or Quit, and cancelling a long
  load without selecting another segment. MAX speed off the UI thread.

## Coverage

- **A coverage report that can be committed.** `dj report --write` (or a step of `dj run`)
  writes a `report/` folder in the project that goes into version control, so a
  collaborator can see what is covered, and the overall %, from a fresh clone without
  hydrating anything, and review coverage changes in a diff. The folder splits the headline
  from the detail:
  - `report/README.md` (or `summary.md`): the headline numbers (functions, blocks and
    instructions covered, overall %, how many segments counted, dirty or not played), what
    changed since the last report, and the largest gaps. Short enough to read on GitHub.
  - `report/functions.csv`: one row per function with its covered and total blocks and
    instructions.
  - `report/blocks/` (or one `blocks.txt`): the verbose part, which blocks are covered in
    each function, for anyone digging into one area.
  - `report/meta.toml`: what the report was built from: the content keys of the segments it
    counts, the core build and the symbol maps. Tools warn when the report is out of date (a
    segment's content key or a symbol map changed since), and the GUI could show it before
    the local database is filled.

  Open: one line per block keeps diffs readable but grows with the ROM; ranges of covered
  offsets per function would be smaller for N64-sized games. Parity columns join once
  comparison cores exist.
- **Coverage in the GUI:** the Coverage tab ([coverage.md](../docs/coverage.md#in-the-gui)) shows the report, a ROM map
  and live new coverage while playing. It rebuilds the whole report after every edit,
  which takes milliseconds at NES sizes. For large games, keep coverage per segment and
  per function or larger block, sum it in a cache (`coverage_sum`, [CACHE_PLAN.md](CACHE_PLAN.md)), and
  update only the sums for segments whose clean or dirty status changed. Parity colors
  (purple and red) join the map once comparison cores exist.
- **"First runs in" needs its own view.** The function details in the Coverage tab list
  every segment where the function first runs (capped at six today). For code that runs
  every tick (the NMI handler, the main loop) that is every segment in the project, so
  the list is noise. Move it into a separate view built for large result sets, for
  example a searchable table of segment, frame and depth, grouped by root segment, or
  just "earliest, shallowest, and N more", with the full list one click away.
- **Symbol maps during active decompilation.** Symbol maps in the formats a decomp already
  keeps are supported ([coverage.md](../docs/coverage.md#symbol-maps)), so the decomp stays the source of truth. Still to do:
  let `.local/local.toml` point a `[[symbols]]` entry at a file in the decomp's own
  checkout, since that path differs per machine; reload when the file changes, so a
  rename shows on the coverage map and list at once; and maybe an opt-in "Name this
  function" action in the Coverage tab that appends a line to `symbol_addrs.txt` (or
  the `.mlb`). Rolling up regions other than `PRG`, and a MIPS sweep for N64, come
  with the N64 connector.

## Project and CLI

- **Backups without version control.** `dj backup <dir>` (and File > Back up… in
  the GUI) writes the project's shared files to a ZIP: `config.toml`, `segments/`,
  `blobs/`, `symbols/` and `unreachable.txt`, never `.local/` (the database can be
  rebuilt by hydrating) and never the ROM. `dj restore <zip> <dir>` puts them back,
  replacing `segments/` whole so deleted segments come back and new ones go. Useful
  before a big agent-driven edit or an experiment, for authors who don't use git.
  Possible extras: an automatic backup before risky commands (a bulk clean, a large
  agent edit), named backups under `.local/backups/` with a limit on how many are
  kept, and a `diff` between a backup and the current files listed by segment title.
- **Project generator (CLI).** A command that sets up a new project in one go:
  find or ask for the ROM, pick a matching core, write `config.toml`, `.gitignore`,
  a README and an empty root segment, and optionally a symbol map. `dj init` covers the
  bare minimum today; this could be an interactive mode of it or its own command.
- **Shorter segment ids.** Replace UUIDs with something short enough to quote in docs
  and commit messages, for example 8 characters of base32 (40 bits). Tools must check
  the new id against every id in the project when generating one, and try again on
  a collision. A tool that finds two files with the same id refuses to load the
  project. Existing UUIDs stay valid, and the CLI already accepts unique prefixes.
- **Macros and Lua scripts** for active RNG manipulation and pokes, instead of long
  runs of `[WRITE]` lines. They would be stored locally, and segments would refer to
  them by name, as `[RNG]` ([reserved commands](../docs/segments.md#reserved-commands)) already reserves.
- **An MCP server** for agents doing decomp work. It would cover where a block first
  runs, the nearest snapshot to trace from, coverage and parity per function, and
  running a segment. The CLI gets the same abilities, including checking parity for
  a single segment (`dj parity <segment> --against <comparison core>`).

## Comparison and parity

- **Comparison cores in the project.** The baseline stays fixed, but a project
  can list several comparison cores (for example two decomps, or a decomp and
  another emulator) in `config.toml`, in version control, each with a name and
  the core it uses. Parity is tracked per comparison core.
- **Side-by-side comparison** with a second libretro instance, namely a decompilation
  built as a core. Its video output may or may not match. The same inputs drive both,
  and the GUI compares state hashes and video.
- **To discuss: emulator vs decomp mismatches.** When the decomp core and the
  emulator disagree (state hash, RAM, or video), the comparison should stop ticking
  and report the first difference it caught: the frame, the regions or addresses
  that differ, and both values. It should not try to resynchronize. Open questions:
  - What counts as a match when the decomp's RAM layout differs from the original
    game's (a map between addresses, or only watched values)?
  - Should video differences stop the run, or only be logged?
- **Comparison cores can define their own parity.** A decomp may be a port with a completely
  different architecture (a PC port of an N64 game, say), so comparing RAM byte for byte
  means nothing. Let a comparison core's connector decide what a match is: an optional
  function in the connector API ([connectors.md](../docs/connectors.md#the-api)), found
  the same way as the others through the proc address callback, for example
  `bool dj_check_parity(const struct dj_parity_input *in)`. The input would carry pointers
  to the baseline's memory regions (with names and sizes), the frame and tick, and maybe the
  baseline's video frame; the core compares against its own state and returns true for
  parity. It could also fill in a short reason or the first mismatch it found, for the
  report. A core without the function falls back to the default check (RAM while
  stepping, below). Open: how much of the baseline to hand over each frame without
  slowing stepping down, and whether the check runs every frame or only at ticks.
- **Open question: how parity is checked.** Nothing is decided yet. The options are:
  - **RAM while stepping.** Run both cores frame by frame and compare RAM (or the
    watched regions) after each frame. It's cheap, and it works with any core. A
    block run on a frame where everything matched gets parity. A mismatch flags
    everything executed on that frame as a suspect, but doesn't say which block
    caused it. It also needs the decomp's RAM layout to match, or an address map.
  - **Tracing.** Hook function entry and exit in both cores (using the symbol maps)
    and compare arguments, return values and memory writes for each call. It finds
    the exact function that diverges, but it's costly, and it needs a mapping
    between baseline addresses and decomp functions.
  - A likely path is RAM while stepping to find the first frame that differs, then
    tracing just that frame to find the function. That also fits the plan of
    stopping at the first mismatch and reporting it.

## Code

- **Refactor the Rust before release (maybe).** Once the tool is close to a first working
  release, consider splitting the longest files by feature. `gui/src/main.rs` is about
  3,000 lines and holds playback, editing, undo, the tree, hydration and the transport
  bar. It could follow `roll.rs` and `cov.rs` into modules such as `playback.rs`,
  `edit.rs`, `tree.rs` and `transport.rs`. In the player, `format.rs` and `libretro.rs`
  are the next largest. This changes no behaviour, so it may not be worth doing at all.
  It also pairs naturally with the terminology rename below.
- **Terminology in the code.** The docs and the GUI use the
  [terms in concepts.md](../docs/concepts.md#terms). The code and the CLI's help still say "replay", "collection"
  and "trunk" in places, and segment files start with `dj-replay 1` (`dj-segment 1`
  would match the `.segment` extension; the tools would read both).
