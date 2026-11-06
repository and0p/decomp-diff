# Example: Thwaite

A small project for [Thwaite](https://github.com/pinobatch/thwaite-nes),
a free NES game by Damian Yerrick (GPL-3.0). It is the end-to-end test for
decomp-dj: 12 segments that boot the game, go through every menu, play,
pause, lose, use a SNES mouse, and include one poked segment.

```
Boot, skip the legal screen
  1 Player: start a game
    Click through the opening cutscene
      Defend: sweep the cursor and fire both silos
      Idle: let the missiles fall            (ends in game over)
      Jump to Saturday (poked)               ([WRITE] RAM 0x3DD = day 6)
      Pause and resume
  2 Players: start a game
  Mouse in port 2: click 1 Player
  Practice: open the menu
    Practice: cancel back to the title
    Practice: play the default wave
```

## Run it

```sh
./build-rom.sh                                   # ROM + symbols into build/
../../connectors/mesen/build.sh                  # core with the coverage connector
cd ../../player && cargo build --release && cd -
E=../../player/target/release/dj
$E run . --rom build/thwaite.nes --core ../../build/cores/mesen_libretro.so   # .so/.dylib/.dll
$E report . --gaps gaps.csv
```

Every segment has recorded start and end hashes, so `run` doubles as a
determinism check: on a matching setup each line reads `ok`. Add
`--from-scratch` to replay every chain from power-on instead of from cached
parent states.

`[tick]` in `config.toml` counts executions of `doStateAction`, the
routine Thwaite runs once per game-logic step. Menus show 0 ticks, and the
pause segment shows 130 ticks over 300 frames.

The `Mouse` segment sends mouse input on `P1` although the mouse is plugged
into port 2: the Mesen core reads mouse motion from libretro port 1 whatever
port the mouse is in.
