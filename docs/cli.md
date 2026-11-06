# The CLI

`dj` is the headless player. It plays segment trees on any libretro core, checks state
hashes, fills the local database and reports coverage. Building it is in
[install.md](install.md).

| Command | What it does | More |
| --- | --- | --- |
| `dj init <dir> --name NAME --rom ROM --core CORE [--option KEY=VALUE]... [--symbols FILE]` | Creates a project and pins the setup in `config.toml` | [install.md](install.md#a-first-project), [config.md](config.md) |
| `dj new <dir> [--parent ID] [--title TEXT] [--frames N]` | Adds a segment and prints its path | [editing.md](editing.md) |
| `dj status <dir>` | The tree with clean, dirty and unverified marks | [playback.md](playback.md#clean-dirty-and-unverified) |
| `dj play <dir> <id> [--screenshots DIR] [--shot 60,120] [--no-cache] [--accept]` | Plays one segment and its chain | [playback.md](playback.md#checking-hashes) |
| `dj run <dir> [--from-scratch] [--accept] [--screenshots]` | Plays everything, checks hashes, records coverage | [playback.md](playback.md#checking-hashes) |
| `dj hydrate <dir> [SEGMENT [--children]] [--clean] [--force]` | Fills the local database | [database.md](database.md#from-the-command-line) |
| `dj report <dir> [--json FILE] [--gaps FILE.csv]` | Coverage of the current, clean segments | [coverage.md](coverage.md#from-the-command-line) |
| `dj where <dir> PRG:0x1234 \| 0x8123 \| SYMBOL` | Which segments first run that code, at which frame | [coverage.md](coverage.md#from-the-command-line) |
| `dj snapshot <dir> SEGMENT FRAME --out FILE` | Writes the core state at that frame, to start a trace from | [coverage.md](coverage.md#from-the-command-line) |
| `dj sql <dir> "SELECT ..."` | Queries the local database | [database.md](database.md) |
| `dj cache stats <dir>` / `dj cache gc <dir>` | Database size, or drop what no segment uses | [database.md](database.md#from-the-command-line) |

Common to every command:

- Ids can be shortened to any unique prefix.
- `--rom` and `--core` are remembered per project in `.local/local.toml`, so later commands don't need them.
- Every command that plays takes `--no-db` to run without the local database.
