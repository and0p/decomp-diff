# Install

decomp-dj is built from source. There are three parts: the CLI, the GUI and a patched
core that reports coverage.

```sh
cd player && cargo build --release        # Rust 1.75+; the binary is player/target/release/dj
cd gui && cargo build --release           # Rust 1.95+ (egui 0.37); the binary is gui/target/release/dj-gui
connectors/mesen/build.sh                 # git, make, a C++11 compiler; output in build/cores/
```

The core is Mesen (NES), patched as described in
[connectors/mesen/README.md](../connectors/mesen/README.md). Any other libretro core plays
segments too, but records no coverage until it has a [connector](connectors.md).

## Trying the example

[examples/thwaite/](../examples/thwaite/) is a 12-segment project for Thwaite, a free NES
game. Its README walks through it; in short, from the repository root:

```sh
(cd examples/thwaite && ./build-rom.sh)          # the ROM and symbols, into examples/thwaite/build/
dj run examples/thwaite --rom examples/thwaite/build/thwaite.nes --core build/cores/mesen_libretro.so   # .so/.dylib/.dll
dj report examples/thwaite
gui/target/release/dj-gui examples/thwaite
```

## A first project

```sh
dj init my-game --name "My Game" --rom game.nes --core build/cores/mesen_libretro.so
dj new my-game --title "Boot to title" --frames 240
```

`init` writes `config.toml` with every core option pinned, remembers the ROM and core
paths in `.local/local.toml`, and creates a `.gitignore` that lists `.local/` when the project has none. Pass `--symbols FILE` to
add a symbol map ([coverage.md](coverage.md#symbol-maps)). Then open the project in the
GUI to record inputs, or edit the segment file by hand ([segments.md](segments.md)).
