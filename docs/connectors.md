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

## Traces

A connector may also export a trace hook, which the [MCP server](mcp.md#traces) uses to
record every instruction and memory access over a span of frames. The player looks these up
too, and treats a core without them as unable to trace.

```c
bool   dj_trace_start(size_t capacity);   // start recording, keeping at most capacity events
void   dj_trace_stop(void);
size_t dj_trace_read(const struct dj_trace_event **events, uint64_t *dropped);
                                          // events since the last read; clears them

struct dj_trace_event {      // 40 bytes
    uint64_t cycle;
    int64_t  offset;         // offset in the region, -1 if none
    uint16_t addr;           // CPU address
    uint8_t  kind;           // 1 execute, 2 read, 3 write
    int8_t   region;         // index as in dj_get_region, -1 if none
    uint8_t  value;          // the byte read or written, or the opcode
    uint8_t  a, x, y, sp, p; // registers before an executed instruction
    uint8_t  operand_count;
    uint8_t  operands[3];
    uint8_t  reserved[4];
};
```

Events past the capacity are counted in `dropped` rather than stored. Only the Mesen
connector has the hook so far.

## Writing one for another core

1. Find where the core fetches an instruction byte, and where it can turn a CPU address into a ROM offset (Mesen's mappers already do: `BaseMapper::GetAbsoluteAddressAndType`).
2. Set the opcode flag on the first byte of each instruction and the operand flag on the rest. Count a tick when the opcode fetch matches the tick location.
3. Expose the functions above through the proc address callback.
4. Check that state hashes are identical with and without the patch, on every segment of a project, and measure the slowdown (about 5% for Mesen).
5. Pin the upstream commit and ship a `build.sh` and the patch under `connectors/<core>/`, like Mesen's.

Rolling coverage up into functions and blocks also needs a sweep for the core's CPU. Only
the 6502 has one so far ([coverage.md](coverage.md#limits)).
