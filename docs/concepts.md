# Concepts

decomp-dj keeps a **project**: a tree of plain-text input **segments** that drive a
libretro core from power-on (or from a saved starting point) into every situation a game
can reach. A project is what people share. Everything else (snapshots, coverage,
previews) is derived locally by playing it.

## Terms

These are the working terms. The docs and the GUI use them. The code and the CLI's help
still use older words in places ("replay" for segment, "collection" for project, "trunk"
for root segment), and segment files still start with `dj-replay 1`.

| Term | Meaning |
|---|---|
| **Project** | One game's folder: `config.toml`, the segments in `segments/`, blobs and `unreachable.txt`. It is what people share and keep in version control. Local data lives in its `.local/` folder. (Formerly "collection".) See [segments.md](segments.md#project-layout). |
| **Segment** | One `.segment` file: inputs by frame, metadata, hashes and a parent link. It is one entry in the GUI's sidebar tree. (Formerly "replay", and "node" in the GUI.) |
| **Root segment** | A segment with no parent. It starts from power-on, or from an SRAM or snapshot blob. (Formerly "trunk".) |
| **Child segment / children** | Segments that start where their parent ends. A segment with no children is a leaf. |
| **Snapshot** | A serialized emulator state, more or less a "save state". It may leave out memory that doesn't help the tools. |
| **Diff** | A patch from one snapshot to a later one, storing only what changed. |
| **Save** | Only the game's own saves: SRAM, memory cards and cartridge save chips. Never an emulator save state. |
| **Marker** | A named point in a segment: an `[EVENT]`, or another line the GUI shows in the marker lane (writes, resets, device changes). |
| **Macro** | Future: anything an author can script, for example RNG manipulation or pokes, kept locally and referred to by name. |
| **State hash** | A hash of the game's memory that says "where the game is" without shipping memory. See [playback.md](playback.md#state-hashes). |
| **Clean / dirty / unverified** | Whether a segment's recorded hashes still hold. Worked out from the hashes, never stored. See [playback.md](playback.md#clean-dirty-and-unverified). |
| **Hydrate** | Play a project (or part of it) to fill the local database with snapshots, diffs, previews and coverage. See [database.md](database.md). |
| **Content key** | A hash of a segment's setup, start state and inputs. Derived data in the local database is filed under it. |
| **Baseline** | The project's reference core (the original emulator) and the results it produces from the inputs. Also called the oracle. There is one per project, and it rarely changes. |
| **Comparison core** | Any other core played against the baseline with the same inputs, usually a decomp built as a libretro core, but possibly another emulator. Working name; "candidate" is another option. A project can list more than one. |
| **Coverage connector** | A small patch to a core that reports which ROM bytes ran. See [connectors.md](connectors.md). |
| **Coverage** | An instruction or block counts as covered when at least one clean segment executed it. Dirty segments don't count. |
| **Uncovered** | In the ROM, not executed by any clean segment, and not marked unreachable. This is where the next segments should go. |
| **Parity** | A comparison core matched the baseline every time the baseline executed an instruction or block. If parity hasn't been checked, a covered block stays covered. |
| **No parity** | A comparison core has been shown to differ from the baseline at an instruction or block. |
| **Unreachable** | An instruction or block that is in the ROM but has been determined to be unreachable. Listed in `unreachable.txt`, in version control. See [coverage.md](coverage.md#unreachable-code). |

## Colors

The GUI, reports and any export use the same colors:

| Color | Meaning |
| --- | --- |
| Green | Covered, and a clean segment (the same green) |
| Purple | Parity |
| Red | No parity |
| Blue | Unreachable |
| Default text color | Uncovered |
| Orange | A dirty segment |
| Gray | An unverified segment |
