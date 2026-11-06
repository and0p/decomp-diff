# decomp-dj project format, version 1

This document defines the files that make up a **project**: a tree of
plain-text input segments that drive a libretro core from power-on (or from a
saved starting point) into every situation a game can reach. A project is
what people share. Everything else (snapshots, coverage databases,
screenshots) is derived locally by playing it.

Status: draft v1. The headless player (`dj`) and the desktop editor
(`dj-gui`) in this repository implement everything here except items
marked **(reserved)**, which files may carry but the tools do not act on yet.

## Terminology

These are the working terms (open to change). The docs and the GUI use them. The code
and the CLI's help still use older words in places ("replay" for segment, "collection"
for project, "trunk" for root segment), and segment files still start with `dj-replay 1`.

| Term | Meaning |
|---|---|
| **Project** | One game's folder: `config.toml`, the segments in `segments/`, blobs and `unreachable.txt`. It is what people share and keep in version control. Local data lives in its `.local/` folder. (Formerly "collection".) |
| **Segment** | One `.segment` file: inputs by frame, metadata, hashes and a parent link. It is one entry in the GUI's sidebar tree. (Formerly "replay" in this spec and "node" in the GUI.) |
| **Root segment** | A segment with no parent. It starts from power-on, or from an SRAM or snapshot blob. (Formerly "trunk".) |
| **Child segment / children** | Segments that start where their parent ends. A segment with no children is a leaf. |
| **Snapshot** | A serialized emulator state, more or less a "save state". It may leave out memory that doesn't help the tools. |
| **Diff** | A patch from one snapshot to a later one, storing only what changed. |
| **Save** | Only the game's own saves: SRAM, memory cards and cartridge save chips. Never an emulator save state. |
| **Marker** | A named point in a segment: an `[EVENT]`, or another line the GUI shows in the marker lane (writes, resets, device changes). |
| **Macro** | Future: anything an author can script, for example RNG manipulation or pokes, kept locally and referred to by name. |
| **Hydrate** | Play a project (or part of it) to fill the local database with snapshots, diffs, previews and coverage. |
| **Content key** | A hash of a segment's setup, start state and inputs. Derived data in the local database is filed under it (see CACHE_PLAN.md). |
| **Baseline** | The project's reference core (the original emulator) and the results it produces from the inputs. Also called the oracle. There is one per project, and it rarely changes. |
| **Comparison core** | Any other core played against the baseline with the same inputs, usually a decomp built as a libretro core, but possibly another emulator. Working name; "candidate" is another option. A project can list more than one. |
| **Coverage** | An instruction or block counts as covered when at least one clean segment executed it. Dirty segments don't count. Shown **green**, the same green as a clean segment. |
| **Uncovered** | In the ROM, not executed by any clean segment, and not marked unreachable. This is where the next segments should go. Shown in the default text color. |
| **Parity** | A comparison core matched the baseline every time the baseline executed an instruction or block. Shown **purple**. If parity hasn't been checked, a covered block stays green. |
| **No parity** | A comparison core has been shown to differ from the baseline at an instruction or block. Shown **red**. |
| **Unreachable** | An instruction or block that is in the ROM but has been determined to be unreachable. Listed in `unreachable.txt`, in version control (section 9). Shown **blue**. |

Colors in summary: green = covered, purple = parity, red = no parity, blue =
unreachable, default = uncovered. These are the same in the GUI, reports and any
export.

## 1. Project layout

```
my-project/
  config.toml            pinned setup: ROM hash, core build, core options, hash rules
  segments/
    7f3c…e1.segment      one file per segment, named by its id
    0a9b…44.segment
  blobs/
    9a1c…7e.srm          SRAM and other blobs, named by content hash
  symbols/               optional: symbol maps for rolling coverage up into functions
  unreachable.txt        optional: ROM ranges no input can reach, left out of coverage
  .local/                local only, never committed
    local.toml           local settings
    cache.db             the local DB
```

Segments are stored **flat**, named by id. The tree lives in each file's
`parent` header, and tools present it as a tree. This keeps paths short, makes
re-parenting a one-line change, and never creates folders for empty leaves.

Nothing in a project is game data, except blobs a project author chooses
to include (SRAM and snapshots). Whether those are fine to distribute is the
author's call.

## 2. config.toml

```toml
format = 1
name = "Thwaite"

[rom]
# Hash of the ROM file exactly as loaded by the core.
sha256 = "c1a0…"
file = "thwaite.nes"        # a hint for humans; the hash is what counts

[core]
name = "mesen"                      # libretro core name
build = "mesen@0102910c+dj1"        # free-form id of the exact build
sha256 = ""                         # optional: hash of the core library (per OS, so usually left empty)

[core.options]                       # every option the core reads, pinned
mesen_ramstate = "All 0s"

[[ports]]                            # controller device per port at power-on
port = 1
device = "JOYPAD"

[hash]
algorithm = "blake3"
regions = ["RAM", "SRAM"]          # memory regions that make up a state hash (section 6)

[tick]                               # optional, section 7
exec = "PRG:0x0123"

[[symbols]]                          # optional, section 9
file = "symbols/thwaite.dbg"
format = "ca65-dbg"
```

**One pinned setup per project.** State hashes are only meaningful for
one core build, one ROM, and one set of core options. A different build is a
different project (or a re-validation of this one, see section 8).

## 3. Segment files

A segment is UTF-8 text with Unix or Windows line endings. It has a header,
a line containing only `---`, and a body. Lines starting with `#` are
comments everywhere. Blank lines are ignored. Tools that rewrite a file keep
every comment, but may move comments in the body to the top of the body.

```
dj-replay 1
id: 3b8f0f2c-1d44-4c1e-9e55-0d7a3c1b2e90
parent: none
title: Boot to title screen
frames: 240
start-frame: 0
sram: 9a1c…
start-hash: 2f4d…
end-hash: 71ee…
---
# frame  command
60  P1 JOYPAD START 1
64  P1 JOYPAD START 0
120 [EVENT] Title screen visible
```

### 3.1 Header fields

| Field | Required | Meaning |
| --- | --- | --- |
| `dj-replay 1` | yes | First line. Magic and format version. |
| `id` | yes | Lowercase UUID. Never changes once published. The file name is `<id>.segment`. |
| `parent` | yes | Id of the segment that must play immediately before this one, or `none` for a root segment. |
| `title` | no | One line for humans and agents. |
| `frames` | yes | Length in frames. The segment ends after this many frames even if the last input is earlier, so no trailing "wait" lines are needed. |
| `start-frame` | no | Frames since power-on when this segment starts (the sum of its ancestors' `frames`, or the frame recorded with a `snapshot` blob). Written by tools; used to catch mistakes. |
| `snapshot` | root only | Blob id of a snapshot to load before frame 0. (Formerly `save`; tools still read that name.) |
| `sram` | root only | Blob id of battery-backed save RAM to load before frame 0. Use it with or without `snapshot`; when both are given, `sram` is applied after the state loads (for cores whose states leave SRAM out). |
| `start-hash` | no | State hash at frame 0, before any input (section 6). |
| `end-hash` | no | State hash after the last frame. |
| `checkpoint` | no | Repeatable: `checkpoint: <frame> <hash>`. The state hash at the start of frame `<frame>`, before that frame's lines apply (so `checkpoint: 0` would equal `start-hash`). Narrows down where a run diverged. Editors check checkpoints while playing and keep the ones before an edit. |
| `tags` | no | Space-separated free tags (`boss`, `rng-forced`, …). |

A root segment with neither `snapshot` nor `sram` is a cold boot with no save data.

Unknown header keys are kept by tools and ignored by players, so the format
can grow without breaking older files.

### 3.2 Frames

A **frame** is one call to the core's `retro_run()`. Body lines start with a
frame number **relative to the start of this segment**, from `0` to
`frames - 1`. Lines must be in non-decreasing frame order; lines on the same
frame apply top to bottom.

Everything on frame `n` is applied before `retro_run()` is called for frame
`n`. So an input set on frame `n` is what the core sees when it polls during
frame `n`.

Every segment starts with all inputs released, whatever its parent was holding
at its end. This keeps a segment's meaning the same whether a player reaches
its start by playing the parent or by loading a cached state.

### 3.3 Input lines

```
<frame> P<port> <DEVICE> <control...> <value>
```

Input is stored the way libretro asks for it: a core calls
`input_state(port, device, index, id)` and gets back a 16-bit value. The line
names those four things and the value. Inputs are **sparse state changes**: a
value holds until a later line changes it. Pressing and releasing a button is
two lines, which is the "hold start, hold end" model.

| Device | Line | Value |
| --- | --- | --- |
| Joypad | `P1 JOYPAD A 1` | `1` pressed, `0` released. Buttons: `B Y SELECT START UP DOWN LEFT RIGHT A X L R L2 R2 L3 R3`. |
| Analog stick | `P1 ANALOG LEFT X -32768` | Signed 16-bit, `-32768..32767`. Sticks `LEFT RIGHT`, axes `X Y`. |
| Analog button | `P1 ANALOG BUTTON R2 32767` | `0..32767` pressure, using joypad button names. |
| Lightgun | `P2 LIGHTGUN SCREEN_X 1200` | Absolute screen position, `-32767..32767` maps edge to edge (libretro's convention). Also `SCREEN_Y`. |
| Lightgun buttons | `P2 LIGHTGUN TRIGGER 1` | `TRIGGER RELOAD AUX_A AUX_B AUX_C START SELECT DPAD_UP DPAD_DOWN DPAD_LEFT DPAD_RIGHT IS_OFFSCREEN`. |
| Mouse | `P2 MOUSE LEFT 1` | Buttons `LEFT RIGHT MIDDLE BUTTON_4 BUTTON_5`. |
| Mouse motion | `P2 MOUSE X 5` | **Impulse**: relative motion for this one frame only, back to `0` on the next frame. Also `Y`, `WHEELUP`, `WHEELDOWN`, `HORIZ_WHEELUP`, `HORIZ_WHEELDOWN`. |
| Pointer / touch | `P1 POINTER X 0` | Absolute, `-32767..32767`. `X Y PRESSED`, with optional finger index: `P1 POINTER 1 X 0`. |
| Keyboard | `P1 KEYBOARD RETURN 1` | Key names are libretro's `RETROK_` names without the prefix. |
| Anything else | `P1 RAW <device> <index> <id> <value>` | Numbers straight from libretro.h, for devices this table misses or core-specific subclasses. |

Ports are numbered from 1 in files (`P1` is libretro port 0).

For analog sticks, a segment may store exact values or only the 8 directions
plus neutral at full deflection (`±32767`). Both are valid. Exact values are
for games that read the precise angle.

### 3.4 Controller devices

```
<frame> [DEVICE] P2 MOUSE
```

Sets the device plugged into a port (`retro_set_controller_port_device`).
The device names are those in the input table, or a number for a
core-specific subclass (`RETRO_DEVICE_SUBCLASS(base, id)` = `((id + 1) << 8) | base`;
Mesen's SNES mouse is `770`). Power-on devices come from `config.toml`,
and a `[DEVICE]` change stays in effect for the rest of the segment and for
all of its descendants.

Cores do not always read input from the port a device is plugged into. Mesen,
for example, reads the mouse from port 1 wherever the mouse is. Segments record
what the core actually polls, so such quirks show up in the file; a comment
line saying so helps the next reader.

### 3.5 Memory writes

```
<frame> [WRITE] <region> <address> <byte> [<byte>...]
```

Writes bytes before the frame runs, starting at `<address>` and going up one
address per byte. Addresses and bytes are hex with or without `0x`. A
contiguous block is one line, however long:

```
0 [WRITE] RAM 0x0300 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F 10
```

Several writes on one frame apply top to bottom, so a later line wins where
two overlap. Regions:

| Region | libretro source |
| --- | --- |
| `RAM` | `retro_get_memory_data(RETRO_MEMORY_SYSTEM_RAM)` |
| `SRAM` | `RETRO_MEMORY_SAVE_RAM` |
| `VRAM` | `RETRO_MEMORY_VIDEO_RAM` |
| `RTC` | `RETRO_MEMORY_RTC` |
| `BUS` | the emulated address bus, through the core's memory maps (`RETRO_ENVIRONMENT_SET_MEMORY_MAPS`) |

A segment with any `[WRITE]` line is **poked**: it reaches a state real play
may not. Tools count its coverage separately and tag it `poked`, so a
project can always say how much coverage comes from play alone.

### 3.6 Events

```
<frame> [EVENT] Mario lands on the flagpole
<frame> [EVENT @128,96] Missile hits the left silo
```

Annotations for humans and agents. The optional `@x,y` is a pixel position
in the core's video output for that frame. Events change nothing in the
emulation, so adding, editing or removing one never changes a hash. A frame
may carry any number of events. Editors show the event text on the timeline
and group lines that share a frame.

### 3.7 Reserved commands

| Command | Meaning |
| --- | --- |
| `[RESET]` | Soft reset (`retro_reset`). |
| `[RNG] <name> <n>` | Advance a named RNG `n` times. Game-specific, defined by the project. |
| `[SCREENSHOT]` | Ask tools to keep this frame's video output. |

Players must reject unknown bracketed commands rather than silently skip them,
because skipping a state-changing command would make every hash after it wrong.

## 4. Blobs

`blobs/` holds SRAM and snapshot files, each named by its content hash (section 6)
and an extension for its kind: `<hash>.srm` for SRAM (libretro's own name for save
RAM files, so other emulators can open them) and `<hash>.state` for a core state.
The header field that names a blob (`sram` or `snapshot`) says which it is. Many segments
share one blob (for example a blank save).
A blob is the exact byte layout the pinned core uses: `retro_serialize`
output for `snapshot`, and the `RETRO_MEMORY_SAVE_RAM` contents for `sram`.

## 5. Playing a segment

To play segment `R`:

1. Walk `parent` links from `R` up to its root segment.
2. Load the ROM, apply core options and port devices from `config.toml`.
3. If the root segment has `snapshot`, `retro_unserialize` it. If it has `sram`, copy it
   into save RAM.
4. For each segment from the root segment down to `R`: check `start-hash` if present,
   apply each frame's lines and call `retro_run()`, check `checkpoint` and
   `end-hash` if present.

Players may skip step 4 for ancestors by loading a cached local state taken
at an ancestor's end. They may do so only if that state was produced by this
same setup, its hash matches the ancestor's `end-hash`, and the ancestor is
clean (section 8), so nothing above it has changed since. Otherwise they play
the chain from the root segment.

A player that has just finished a segment may go straight on into one of its
children without reloading anything. The core is already in the child's start
state; the player only releases all inputs and sets port devices for the child
(section 3.2).

## 6. State hashes

A state hash identifies "where the game is" without shipping memory.
Every hash in the format (state hashes, checkpoints and blob names) is the first 32
hex digits (128 bits) of a BLAKE3 digest, with no prefix: the format version pins the
algorithm, so changing it would be a new version. A state hash is the digest of, for
each region in `[hash].regions` in order:

```
region name (ASCII) | 0x00 | size as u64 little-endian | region bytes
```

Only memory the core exposes through `retro_get_memory_data` (or the
memory maps, for `BUS:<start>-<end>` ranges) can be hashed. System RAM is
the usual choice. Snapshot blobs themselves are not hashed for this
purpose, because they hold emulator internals (timers, audio buffers) that
change with core versions even when the game is in the same place.

A segment's `start-hash` must equal its parent's `end-hash`.

## 7. Logic ticks (draft)

Frame numbers are the clock, but they are not always the game's clock. Games
that run their logic once per vertical blank (most 8- and 16-bit games) have
one logic tick per frame. Others (most N64 games, and any game that lags) run
logic at their own rate, so "frame 812" does not mean "tick 812".

A project can declare what counts as one logic tick:

```toml
[tick]
exec = "PRG:0x1F3A"       # a tick is each execution of this ROM location
# or (reserved): write = "RAM:0x0021"  # each write to this address
```

The coverage connector counts ticks while the core runs. Tools report ticks
next to frames (`frame 812, tick 790`), so events and parity checks can be
lined up by tick. In the Thwaite example, the tick is `doStateAction`: menus
run 0 ticks, and a 300-frame segment that pauses the game runs 130.

Only `exec` is implemented in v1; `write` is **(reserved)**.

Input lines stay keyed by frame in v1, because that is what a core consumes. Keying input by tick is **(reserved)** for a later version
once it has been tried on a multithreaded game.

## 8. Clean and dirty segments

A segment is:

- **unverified** if it has no `end-hash`.
- **clean** if it has an `end-hash` and its `start-hash` (if any) equals its
  parent's `end-hash`.
- **dirty** if its `start-hash` differs from its parent's current `end-hash`.
  This happens when a parent is edited and re-recorded.

A segment below a dirty segment is also dirty, even when its own hashes agree
with its parent's: its start state rests on something that has changed.

Dirty is never stored. Tools work it out from the hashes. Re-validating a
dirty segment means playing it from its parent's new end state and, if it still
does what it should, writing the new `start-hash` and `end-hash`. Its children
then become dirty in turn, and so on down the tree.

When a segment is edited, tools remove its `end-hash` and every `checkpoint`
after the first edited frame, because those states are no longer known. The
segment becomes unverified and everything below it dirty, until it is played
and the new hashes are accepted. Editing only `[EVENT]` lines changes no
state, so it keeps every hash.

## 9. Coverage (local, derived)

Coverage is not part of the shared files. Players record it locally while
segments run, through a per-core **coverage connector** (see
`connectors/README.md`). The connector reports executed locations as
**offsets into the ROM image**, never CPU addresses, because bank switching
maps different ROM bytes to the same CPU address.

For each segment, the player stores which ROM bytes ran as an instruction's
first byte (opcode) and which ran as operands, during that segment's own frames
only. Ancestors' frames are not counted again.

It keeps this in the local database (`.local/cache.db`, see CACHE_PLAN.md)
keyed by the segment's **content key**, so the totals always describe the
current files: a report counts only clean segments whose current content has
been played, and says how many were dirty or not played yet. Each segment also
records the frame at which each ROM byte first ran in it, which answers "which
segment reaches this code, and when".

`unreachable.txt` at the project root lists ROM ranges that no input can
reach (dead code, debug paths). It is version controlled. One range per line,
`0x8000-0x80FF` or a single `0x8123`, optionally prefixed by a region
(`PRG:0x8000-0x80FF`), with `#` comments. Instructions and blocks inside them
are left out of the totals.

Coverage rolls up into basic blocks and functions when a symbol map is given:

- `ca65-dbg`: the `.dbg` file from the cc65 linker (`ld65 --dbgfile`). Each
  `.proc` scope becomes a function, and so does each top-level label in a
  segment whose name contains `CODE`, up to the next label. `rom_header`
  (default 16, the iNES header) is subtracted from file offsets to get PRG
  offsets.
- `csv`: one function per line, `name,region,offset,size` (offset and size in hex).

For 6502 code the player splits each function into instructions and basic
blocks by a linear sweep (a block ends at a branch, jump, return or `BRK`, and
starts at a branch or jump target). A block counts as covered when its first
instruction ran. Data placed inside a function's range can confuse the sweep.

Coverage can be exported as a gap list (functions and blocks no segment
reaches) to show collaborators where help is needed.

## 10. Editing the tree

These are the tree edits the GUI offers. They are defined here so that any
tool doing the same edit produces the same files.

**New child.** A new segment with `parent` set to the segment, a fresh id, no
hashes, and a length chosen by the author. It starts where the segment ends.

**Split at frame `f`** (`0 < f < frames`). One segment becomes two:

- The **second part** keeps the segment's id, title, tags, children and
  `end-hash`. Its body is the segment's lines from frame `f` on, renumbered from 0,
  and its `frames` is the old length minus `f`. Its `parent` becomes the first
  part. Checkpoints after `f` move to it, renumbered. It loses `snapshot` and
  `sram`, because only a root segment may name blobs.
- The **first part** gets a new id and takes the segment's old place under its
  parent. It keeps frames `0..f`, the earlier checkpoints, and any `snapshot` or
  `sram`. By default its title is the old title plus "(first f frames)".
- The state hash at the cut (at the start of frame `f`, after the first part
  has run) becomes the first part's `end-hash` and the second part's
  `start-hash`. Because the same frames still run with the same inputs, the
  second part and everything below it stay clean.
- Every segment starts with all inputs released (section 3.2). So for each
  input held across the cut, the second part gets a line at frame 0 that sets
  it again.

Keeping the id on the second part means children and outside references to the
segment stay valid. Coverage is stored by content key (section 9), so both parts
get new keys and their coverage is recorded again the next time they are played.

**Branch at frame `f`.** A split at `f`, plus a new empty child of the first
part, so that a second path starts at the cut.

**Join with parent** (only for a segment that is its parent's only child), the
inverse of a split. The parent keeps its id, title and place, and grows by the
child's `frames`. The child's lines follow the parent's, shifted by the parent's old
length, with a release at the join for any input the parent still held (as a segment
start would do). The child's children become the parent's, the parent takes the
child's `end-hash` and later checkpoints, and the child's `start-hash` becomes a
checkpoint at the join frame. The same frames run with the same inputs, so every hash
below stays valid. Tools may add an `[EVENT]` at the join frame, by default named
after the child, to keep the moment the split marked.

**Delete.** Removes a segment and everything below it.

**Properties.** Changing `title` changes no state. Changing `frames` drops the
`end-hash` and any checkpoints past the new end. A segment cannot be shortened
past its last body line.

## 11. Future directions (notes, not part of format v1)

- **Rewinding up the chain.** Stepping or seeking back past frame 0 goes into the
  parent's last frames (loading a snapshot of the parent, from memory or the local
  database), and playing forward from there follows the sticky segments back down. A
  possible next step is one timeline across the whole chain, with the segment
  boundaries drawn on the ruler.
- **Neighbours on the piano roll.** Draw the end of the parent to the left of frame
  0, and the start of the next (sticky or only) segment to the right of the last frame,
  faded and read-only, with the boundary marked on the ruler. Clicking or dragging
  in the faded part does nothing, so a misclick can't switch segments. Only an arrow
  key or a playback control (stepping, playing, the start and end
  buttons) moves across the boundary, and that switches over fully: the other
  segment becomes the selected one, the playhead moves with it even when paused, and
  only then can it be edited. No edit ever spans two segments.
- **Drag to draw in the piano roll.** Today a click makes a press as long as the
  last one made or resized, and a drag on empty space selects. Holding the button down
  on an empty cell and dragging could set the new press's length instead, with
  selecting moved to a modifier or to drags that start outside the rows.
- **Return to the edit.** An option (on by default, to try) so that any edit pauses
  playback and moves the playhead to the earliest frame edited since playback last
  ran, showing that frame recomputed. Today the GUI drops the snapshots after the edit
  and recomputes the frame under the playhead, which is correct but hard to trust,
  because the picture doesn't visibly change and may be far from the edit. Also: mark
  the frames after an unplayed edit as stale on the ruler until they run again, and
  dim the picture while it shows a frame from before the latest edit.
- **Backups without version control.** `dj backup <dir>` (and File > Back up… in
  the GUI) writes the project's shared files to a ZIP: `config.toml`, `segments/`,
  `blobs/`, `symbols/` and `unreachable.txt`, never `.local/` (the database can be
  rebuilt by hydrating) and never the ROM. `dj restore <zip> <dir>` puts them back,
  replacing `segments/` whole so deleted segments come back and new ones go. Useful
  before a big agent-driven edit or an experiment, for authors who don't use git.
  Possible extras: an automatic backup before risky commands (a bulk clean, a large
  agent edit), named backups under `.local/backups/` with a limit on how many are
  kept, and a `diff` between a backup and the current files listed by segment title.
- **Macros and Lua scripts** for active RNG manipulation and pokes, instead of long
  runs of `[WRITE]` lines. They would be stored locally, and segments would refer to
  them by name, as `[RNG]` (section 3.7) already reserves.
- **Coverage in the GUI:** the Coverage tab (QUICKSTART.md) shows the report, a ROM map
  and live new coverage while playing. It rebuilds the whole report after every edit,
  which takes milliseconds at NES sizes. For large games, keep coverage per segment and
  per function or larger block, sum it in a cache (`coverage_sum`, CACHE_PLAN.md), and
  update only the sums for segments whose clean or dirty status changed. Parity colors
  (purple and red) join the map once comparison cores exist.
- **Comparison cores in the project.** The baseline stays fixed, but a project
  can list several comparison cores (for example two decomps, or a decomp and
  another emulator) in `config.toml`, in version control, each with a name and
  the core it uses. Parity is tracked per comparison core.
- **Side-by-side comparison** with a second libretro instance, namely a decompilation
  built as a core. Its video output may or may not match. The same inputs drive both,
  and the GUI compares state hashes and video.
- **Audio playback (GUI).** Play the core's audio while a segment plays, with a volume
  slider and a mute button in the playback toolbar. When more than one core is running
  (side-by-side comparison), an A|B toggle picks which core you hear.
- **Project generator (CLI).** A command that sets up a new project in one go:
  find or ask for the ROM, pick a matching core, write `config.toml`, `.gitignore`,
  a README and an empty root segment, and optionally a symbol map. `dj init` covers the
  bare minimum today; this could be an interactive mode of it or its own command.
- **Terminology in the code.** The docs and the GUI use the terms in the
  Terminology section. The code and the CLI's help still say "replay", "collection"
  and "trunk" in places, and segment files start with `dj-replay 1` (`dj-segment 1`
  would match the `.segment` extension; the tools would read both).
- **Shorter segment ids.** Replace UUIDs with something short enough to quote in docs
  and commit messages, for example 8 characters of base32 (40 bits). Tools must check
  the new id against every id in the project when generating one, and try again on
  a collision. A tool that finds two files with the same id refuses to load the
  project. Existing UUIDs stay valid, and the CLI already accepts unique prefixes.
- **To discuss: emulator vs decomp mismatches.** When the decomp core and the
  emulator disagree (state hash, RAM, or video), the comparison should stop ticking
  and report the first difference it caught: the frame, the regions or addresses
  that differ, and both values. It should not try to resynchronize. Open questions:
  - What counts as a match when the decomp's RAM layout differs from the original
    game's (a map between addresses, or only watched values)?
  - Should video differences stop the run, or only be logged?
- **An MCP server** for agents doing decomp work. It would cover where a block first
  runs, the nearest snapshot to trace from, coverage and parity per function, and
  running a segment. The CLI gets the same abilities, including checking parity for
  a single segment (`dj parity <segment> --against <comparison core>`).
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
