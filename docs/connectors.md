# Coverage connectors

A coverage connector is a small patch to a libretro core that reports which ROM bytes ran.
Without one, a core still plays segments and checks hashes, but records no coverage and
counts no [ticks](playback.md#logic-ticks-draft). The only connector so far is for Mesen
(NES): [connectors/mesen/](../connectors/mesen/README.md).

## What a connector reports

- Executed locations as **offsets into ROM regions**, never CPU addresses, because bank switching maps different ROM bytes to the same CPU address.
- One flag byte per byte of each region: `0x01` executed as an opcode, `0x02` executed as an operand. Flags only ever get set, until the player clears them.
- Region names the player knows: `PRG` for program ROM is the one coverage rolls up. Mesen also reports `RAM`, `WRAM` and `SRAM`.

## The API

The core registers `RETRO_ENVIRONMENT_SET_PROC_ADDRESS_CALLBACK` and returns these
functions by name. The player looks them up after `retro_init`, and treats a core without
them as having no connector.

```c
uint32_t dj_api_version(void);                       // 1
size_t   dj_region_count(void);
bool     dj_get_region(size_t index, struct dj_region *out);
void     dj_clear(void);                             // zero every flag and the tick count
void     dj_set_tick_exec(int region, int64_t offset); // count executions here; region -1 turns it off; resets the count
uint64_t dj_get_tick_count(void);

struct dj_region {
    const char *name;   // "PRG", "RAM", ...
    uint8_t    *flags;  // one flag byte per region byte, owned by the core
    size_t      size;
};
```

The player calls `dj_clear` when a segment starts and reads the flags as it plays, so
each segment records only its own frames. The GUI also reads and clears them on every
screen refresh for live coverage.

## Writing one for another core

1. Find where the core fetches an instruction byte, and where it can turn a CPU address into a ROM offset (Mesen's mappers already do: `BaseMapper::GetAbsoluteAddressAndType`).
2. Set the opcode flag on the first byte of each instruction and the operand flag on the rest. Count a tick when the opcode fetch matches the tick location.
3. Expose the functions above through the proc address callback.
4. Check that state hashes are identical with and without the patch, on every segment of a project, and measure the slowdown (about 5% for Mesen).
5. Pin the upstream commit and ship a `build.sh` and the patch under `connectors/<core>/`, like Mesen's.

Rolling coverage up into functions and blocks also needs a sweep for the core's CPU. Only
the 6502 has one so far ([coverage.md](coverage.md#limits)).
