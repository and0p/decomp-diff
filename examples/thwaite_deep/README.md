# Thwaite, deep tree

A stress test for the tree view: 570 short segments, nested up to 152 levels deep.
Nothing here plays the game. Every segment sits at Thwaite's main menu, mostly
with no input at all, so the whole project plays in under 20 seconds.

It uses the ROM and symbols from `../thwaite`, so build those first (see that
folder's README).

## Shape

- **A. Boot to the menu, cursor on 1 Player**: a straight chain of 150 idle
  segments (4 to 20 frames each, an event every 25th), plus two side chains that
  tap DOWN and idle on (at depth 40 for 30 segments, at depth 100 for 20 segments).
- **B. Boot to the menu, cursor on 2 Players**: an 80-segment chain that forks
  every 20 levels into three short chains: one taps UP, one taps DOWN, one
  just waits.
- **C. Boot to the menu, cursor on Practice**: a 40-segment stem that fans out
  into three forks, where the first fork forks again, four levels down.

## Run it

```sh
cd examples/thwaite_deep
E=../../player/target/release/dj
$E run . --rom ../thwaite/build/thwaite.nes --core ../../build/cores/mesen_libretro.so
../../gui/target/release/dj-gui .
```

All hashes are recorded, so `run` should report every segment clean.

## Regenerate

`python3 generate.py .` rewrites `segments/` from a fixed random seed (it does
not delete old files, so empty `segments/` first). Then record the hashes with
`run . --accept`.
