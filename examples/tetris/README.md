# Tetris

A minimal collection for trying the GUI on a commercial game: one trunk node
that boots Tetris (USA) and starts an A-type game at level 0.

The ROM is not included. Put your own dump at `build/tetris_usa.nes`
(sha256 `ddb876c3…53082`, checked against `collection.toml`). `build/` is
gitignored.

## The trunk: "Start a game: A-type, level 0" (760 frames)

| Frame | What happens |
|---|---|
| 0–559 | Legal screen (it can't be skipped) |
| 580 | START on the title screen |
| 640 | START on game type (A-type, music 1) |
| 700 | START on level select (level 0); the first piece is falling by 760 |

## Run it

```sh
cd examples/tetris
../../player/target/release/emucov run . --rom build/tetris_usa.nes --core ../../build/cores/mesen_libretro.dylib
../../gui/target/release/emucov-gui .
```

There are no symbols, so coverage reports raw executed bytes only.
