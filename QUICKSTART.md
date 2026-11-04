# Quickstart

What is here so far (first plan, steps 1 to 4):

| Path | What it is |
| --- | --- |
| [SPEC.md](SPEC.md) | The replay collection format, v1 draft |
| [player/](player/) | `emucov`, the headless player (Rust): plays replay trees on any libretro core, checks state hashes, records and reports coverage |
| [connectors/](connectors/) | Per-core coverage patches. [mesen](connectors/mesen/) (NES) is the first |
| [examples/thwaite/](examples/thwaite/) | A 12-replay collection for a free NES game, used as the end-to-end test |

## Build

```sh
cd player && cargo build --release        # Rust 1.75+; the binary is player/target/release/emucov
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
- No UI yet; replays are written by hand (they are plain text) and checked with `play --screenshots`.
