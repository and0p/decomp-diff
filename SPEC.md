# emu_coverage replay format, version 1

This document defines the files that make up a **replay collection**: a tree of
plain-text input replays that drive a libretro core from power-on (or from a
saved starting point) into every situation a game can reach. A collection is
what people share. Everything else (save states, coverage databases,
screenshots) is derived locally by playing it.

Status: draft v1. The headless player (`emucov`) and the desktop editor
(`emucov-gui`) in this repository implement everything here except items
marked **(reserved)**, which files may carry but the tools do not act on yet.

Terms: a **replay** file is one **node** of the tree (the GUI says "node").
A **trunk** has no parent. A **branch** is any node with children, and a
**leaf** has none.

## 1. Collection layout

```
my-collection/
  collection.toml        pinned setup: ROM hash, core build, core options, hash rules
  replays/
    7f3c…e1.replay       one file per replay, named by its id
    0a9b…44.replay
  blobs/
    blake3-<hex>         SRAM and save-state blobs, named by content hash
  symbols/               optional: symbol maps for rolling coverage up into functions
  .emucov/               local only, never committed (states cache, coverage.sqlite, screenshots)
```

Replays are stored **flat**, named by id. The tree lives in each file's
`parent` header, and tools present it as a tree. This keeps paths short, makes
re-parenting a one-line change, and never creates folders for empty leaves.

Nothing in a collection is game data, except blobs a collection author chooses
to include (SRAM and save states). Whether those are fine to distribute is the
author's call.

## 2. collection.toml

```toml
format = 1
name = "Thwaite"

[rom]
# Hash of the ROM file exactly as loaded by the core.
sha256 = "c1a0…"
file = "thwaite.nes"        # a hint for humans; the hash is what counts

[core]
name = "mesen"                      # libretro core name
build = "mesen@0102910c+emucov1"    # free-form id of the exact build
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

**One pinned setup per collection.** State hashes are only meaningful for
one core build, one ROM, and one set of core options. A different build is a
different collection (or a re-validation of this one, see section 8).

## 3. Replay files

A replay is UTF-8 text with Unix or Windows line endings. It has a header,
a line containing only `---`, and a body. Lines starting with `#` are
comments everywhere. Blank lines are ignored. Tools that rewrite a file keep
every comment, but may move comments in the body to the top of the body.

```
emucov-replay 1
id: 3b8f0f2c-1d44-4c1e-9e55-0d7a3c1b2e90
parent: none
title: Boot to title screen
frames: 240
start-frame: 0
sram: blake3-9a1c…
start-hash: blake3-2f4d…
end-hash: blake3-71ee…
---
# frame  command
60  P1 JOYPAD START 1
64  P1 JOYPAD START 0
120 [EVENT] Title screen visible
```

### 3.1 Header fields

| Field | Required | Meaning |
| --- | --- | --- |
| `emucov-replay 1` | yes | First line. Magic and format version. |
| `id` | yes | Lowercase UUID. Never changes once published. The file name is `<id>.replay`. |
| `parent` | yes | Id of the replay that must play immediately before this one, or `none` for a trunk. |
| `title` | no | One line for humans and agents. |
| `frames` | yes | Length in frames. The replay ends after this many frames even if the last input is earlier, so no trailing "wait" lines are needed. |
| `start-frame` | no | Frames since power-on when this replay starts (the sum of its ancestors' `frames`, or the frame recorded with a `save` blob). Written by tools; used to catch mistakes. |
| `save` | trunks only | Blob id of a core save state to load before frame 0. |
| `sram` | trunks only | Blob id of battery-backed save RAM to load before frame 0. Use it with or without `save`; when both are given, `sram` is applied after the state loads (for cores whose states leave SRAM out). |
| `start-hash` | no | State hash at frame 0, before any input (section 6). |
| `end-hash` | no | State hash after the last frame. |
| `checkpoint` | no | Repeatable: `checkpoint: <frame> <hash>`. The state hash at the start of frame `<frame>`, before that frame's lines apply (so `checkpoint: 0` would equal `start-hash`). Narrows down where a run diverged. Editors check checkpoints while playing and keep the ones before an edit. |
| `tags` | no | Space-separated free tags (`boss`, `rng-forced`, …). |

A trunk with neither `save` nor `sram` is a cold boot with no save data.

Unknown header keys are kept by tools and ignored by players, so the format
can grow without breaking older files.

### 3.2 Frames

A **frame** is one call to the core's `retro_run()`. Body lines start with a
frame number **relative to the start of this replay**, from `0` to
`frames - 1`. Lines must be in non-decreasing frame order; lines on the same
frame apply top to bottom.

Everything on frame `n` is applied before `retro_run()` is called for frame
`n`. So an input set on frame `n` is what the core sees when it polls during
frame `n`.

Every replay starts with all inputs released, whatever its parent was holding
at its end. This keeps a replay's meaning the same whether a player reaches
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

For analog sticks, a replay may store exact values or only the 8 directions
plus neutral at full deflection (`±32767`). Both are valid. Exact values are
for games that read the precise angle.

### 3.4 Controller devices

```
<frame> [DEVICE] P2 MOUSE
```

Sets the device plugged into a port (`retro_set_controller_port_device`).
The device names are those in the input table, or a number for a
core-specific subclass (`RETRO_DEVICE_SUBCLASS(base, id)` = `((id + 1) << 8) | base`;
Mesen's SNES mouse is `770`). Power-on devices come from `collection.toml`,
and a `[DEVICE]` change stays in effect for the rest of the replay and for
all of its descendants.

Cores do not always read input from the port a device is plugged into. Mesen,
for example, reads the mouse from port 1 wherever the mouse is. Replays record
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

A replay with any `[WRITE]` line is **poked**: it reaches a state real play
may not. Tools count its coverage separately and tag it `poked`, so a
collection can always say how much coverage comes from play alone.

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
| `[RNG] <name> <n>` | Advance a named RNG `n` times. Game-specific, defined by the collection. |
| `[SCREENSHOT]` | Ask tools to keep this frame's video output. |

Players must reject unknown bracketed commands rather than silently skip them,
because skipping a state-changing command would make every hash after it wrong.

## 4. Blobs

`blobs/` holds SRAM and save-state files, each named by its content hash:
`blake3-<64 hex>`. Many replays share one blob (for example a blank save).
A blob is the exact byte layout the pinned core uses: `retro_serialize`
output for `save`, and the `RETRO_MEMORY_SAVE_RAM` contents for `sram`.

## 5. Playing a replay

To play replay `R`:

1. Walk `parent` links from `R` up to its trunk.
2. Load the ROM, apply core options and port devices from `collection.toml`.
3. If the trunk has `save`, `retro_unserialize` it. If it has `sram`, copy it
   into save RAM.
4. For each replay from the trunk down to `R`: check `start-hash` if present,
   apply each frame's lines and call `retro_run()`, check `checkpoint` and
   `end-hash` if present.

Players may skip step 4 for ancestors by loading a cached local state taken
at an ancestor's end. They may do so only if that state was produced by this
same setup, its hash matches the ancestor's `end-hash`, and the ancestor is
clean (section 8), so nothing above it has changed since. Otherwise they play
the chain from the trunk.

A player that has just finished a node may go straight on into one of its
children without reloading anything. The core is already in the child's start
state; the player only releases all inputs and sets port devices for the child
(section 3.2).

## 6. State hashes

A state hash identifies "where the game is" without shipping memory.
It is `blake3-` plus the hex BLAKE3 digest of, for each region in
`[hash].regions` in order:

```
region name (ASCII) | 0x00 | size as u64 little-endian | region bytes
```

Only memory the core exposes through `retro_get_memory_data` (or the
memory maps, for `BUS:<start>-<end>` ranges) can be hashed. System RAM is
the usual choice. Save state blobs themselves are not hashed for this
purpose, because they hold emulator internals (timers, audio buffers) that
change with core versions even when the game is in the same place.

A replay's `start-hash` must equal its parent's `end-hash`.

## 7. Logic ticks (draft)

Frame numbers are the clock, but they are not always the game's clock. Games
that run their logic once per vertical blank (most 8- and 16-bit games) have
one logic tick per frame. Others (most N64 games, and any game that lags) run
logic at their own rate, so "frame 812" does not mean "tick 812".

A collection can declare what counts as one logic tick:

```toml
[tick]
exec = "PRG:0x1F3A"       # a tick is each execution of this ROM location
# or (reserved): write = "RAM:0x0021"  # each write to this address
```

The coverage connector counts ticks while the core runs. Tools report ticks
next to frames (`frame 812, tick 790`), so events and parity checks can be
lined up by tick. In the Thwaite example, the tick is `doStateAction`: menus
run 0 ticks, and a 300-frame replay that pauses the game runs 130.

Only `exec` is implemented in v1; `write` is **(reserved)**.

Input lines stay keyed by frame in v1, because that is what a core consumes. Keying input by tick is **(reserved)** for a later version
once it has been tried on a multithreaded game.

## 8. Clean and dirty replays

A replay is:

- **unverified** if it has no `end-hash`.
- **clean** if it has an `end-hash` and its `start-hash` (if any) equals its
  parent's `end-hash`.
- **dirty** if its `start-hash` differs from its parent's current `end-hash`.
  This happens when a parent is edited and re-recorded.

A replay below a dirty replay is also dirty, even when its own hashes agree
with its parent's: its start state rests on something that has changed.

Dirty is never stored. Tools work it out from the hashes. Re-validating a
dirty replay means playing it from its parent's new end state and, if it still
does what it should, writing the new `start-hash` and `end-hash`. Its children
then become dirty in turn, and so on down the tree.

When a replay is edited, tools remove its `end-hash` and every `checkpoint`
after the first edited frame, because those states are no longer known. The
replay becomes unverified and everything below it dirty, until it is played
and the new hashes are accepted. Editing only `[EVENT]` lines changes no
state, so it keeps every hash.

## 9. Coverage (local, derived)

Coverage is not part of the shared files. Players record it locally while
replays run, through a per-core **coverage connector** (see
`connectors/README.md`). The connector reports executed locations as
**offsets into the ROM image**, never CPU addresses, because bank switching
maps different ROM bytes to the same CPU address.

For each replay, the player stores which ROM bytes ran as an instruction's
first byte (opcode) and which ran as operands, during that replay's own frames
only. Ancestors' frames are not counted again.

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

Coverage can be exported as a gap list (functions and blocks no replay
reaches) to show collaborators where help is needed.

## 10. Editing the tree

These are the tree edits the GUI offers. They are defined here so that any
tool doing the same edit produces the same files.

**New child.** A new replay with `parent` set to the node, a fresh id, no
hashes, and a length chosen by the author. It starts where the node ends.

**Split at frame `f`** (`0 < f < frames`). One node becomes two:

- The **second part** keeps the node's id, title, tags, children and
  `end-hash`. Its body is the node's lines from frame `f` on, renumbered from 0,
  and its `frames` is the old length minus `f`. Its `parent` becomes the first
  part. Checkpoints after `f` move to it, renumbered. It loses `save` and
  `sram`, because only a trunk may name blobs.
- The **first part** gets a new id and takes the node's old place under its
  parent. It keeps frames `0..f`, the earlier checkpoints, and any `save` or
  `sram`. By default its title is the old title plus "(first f frames)".
- The state hash at the cut (at the start of frame `f`, after the first part
  has run) becomes the first part's `end-hash` and the second part's
  `start-hash`. Because the same frames still run with the same inputs, the
  second part and everything below it stay clean.
- Every replay starts with all inputs released (section 3.2). So for each
  input held across the cut, the second part gets a line at frame 0 that sets
  it again.

Keeping the id on the second part means children and outside references to the
node stay valid. Coverage recorded for the old node (section 9) covered both
parts; tools must mark it stale or re-record it after a split.

**Branch at frame `f`.** A split at `f`, plus a new empty child of the first
part, so that a second path starts at the cut.

**Properties.** Changing `title` changes no state. Changing `frames` drops the
`end-hash` and any checkpoints past the new end. A replay cannot be shortened
past its last body line.
