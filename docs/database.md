# The local database

`.local/cache.db` is a SQLite file that holds what playing the project produced. It is
never committed: everything in it can be rebuilt by playing the project, so each author
fills it on their own machine ("hydrates" it). The design and its reasons are in
[CACHE_PLAN.md](../.agents/CACHE_PLAN.md).

The database is optional. Without it (`--no-db`, or `database = false` in
`.local/local.toml`), the tools play from power-on, or from the nearest snapshot in
memory. It only makes things faster, except for coverage reports, which read it.

## What it holds

Rows are keyed by each segment's **content key**: a hash of its setup, start state and
lines. An edit gives a segment a new key, so it invalidates only that segment's rows.

- **Snapshots** of the core state at frame 0, every `diff_every` frames, at markers, and at the end of segments with children. Each is split into pages that are stored once by hash and compressed with zstd.
- **Previews** (PNG) at the start of each segment and at markers.
- **Coverage** flags for each segment, and the frame at which each ROM byte first ran in it, which `dj xref` reads ([coverage.md](coverage.md)).

How many snapshots and previews it keeps is set by the `[cache]` table and its presets,
in `config.toml` or per machine in `.local/local.toml` ([config.md](config.md#cache-optional)).

## From the command line

```sh
dj hydrate <dir> [SEGMENT [--children]] [--clean] [--force]  # fill the local database
dj cache stats <dir>                                         # size, and how well pages share
dj cache gc <dir>                                            # drop what no segment uses
dj sql <dir> "SELECT ..."                                    # query it directly
```

- `hydrate` plays only the segments whose content isn't stored yet, so running it again is nearly instant. `SEGMENT --children` limits it to a subtree. `--force` plays even what is stored. `--clean` re-checks dirty segments ([playback.md](playback.md#checking-hashes)).
- `cache stats` warns when the file is larger than `budget_gb`.
- `cache gc` deletes contents that no current segment uses, then unused snapshots and pages, and compacts the file.
- Every command that plays takes `--no-db` to run without the database.

## In the GUI

The Emulation menu has **Hydrate all** and **Hydrate and clean all** (only with nothing
unsaved, since cleaning writes files). Right-click in the tree for **Hydrate this
segment** and **Hydrate this and below**. Hydration shows every 4th frame in the picture,
with a progress bar and **Pause** and **Stop** in the status bar. Selecting, seeking and
playing wait until it ends. Segments finished before a stop stay stored.

Segments whose current content is stored show a blue **db** after their length in the
tree. **Use the local database** turns it off for this machine.

### Seeking and memory

- While a segment plays, the GUI keeps a snapshot in memory at the start of every 10th frame.
- A seek starts from the nearest snapshot at or before the target, from memory or from the database, whichever is nearer, then runs the core forward and draws only the last frame. With `diff_every = 60`, a seek anywhere is near instant.
- Selecting a segment starts from its parent's end snapshot in the database when the parent is clean, so deep segments open without playing the chain. The GUI also stores each segment's start snapshot, and the end snapshot of segments with children.
- Long seeks and loads run in slices, so the window stays usable, with a progress bar in the status bar. Selecting another segment cancels one.
- An edit drops the snapshots after the edited frame and redraws the current frame.
- Emulation > **Snapshot memory** (`max_memory_mb`, 512 MB by default) caps in-memory snapshots. Past it, the snapshots farthest from the playhead go first; frame 0 always stays, and anchors every 300 frames stay while they fit. An NES state is a few tens of KB, so NES never reaches it; it matters for cores with large states.
