//! Instruction traces from the connector's trace hook, as text (docs/mcp.md#traces).

use crate::libretro::{TraceEvent, TRACE_EXEC, TRACE_READ, TRACE_WRITE};

#[derive(Clone, Copy)]
enum Mode {
    Imp,
    Acc,
    Imm,
    Zp,
    Zpx,
    Zpy,
    Abs,
    Abx,
    Aby,
    Ind,
    Izx,
    Izy,
    Rel,
}

/// Mnemonic and addressing mode of each 6502 opcode, the stable undocumented ones
/// included (lower case).
fn op_6502(op: u8) -> (&'static str, Mode) {
    use Mode::*;
    const T: [(&str, Mode); 256] = [
        ("BRK", Imp), ("ORA", Izx), ("jam", Imp), ("slo", Izx), ("nop", Zp), ("ORA", Zp), ("ASL", Zp), ("slo", Zp),
        ("PHP", Imp), ("ORA", Imm), ("ASL", Acc), ("anc", Imm), ("nop", Abs), ("ORA", Abs), ("ASL", Abs), ("slo", Abs),
        ("BPL", Rel), ("ORA", Izy), ("jam", Imp), ("slo", Izy), ("nop", Zpx), ("ORA", Zpx), ("ASL", Zpx), ("slo", Zpx),
        ("CLC", Imp), ("ORA", Aby), ("nop", Imp), ("slo", Aby), ("nop", Abx), ("ORA", Abx), ("ASL", Abx), ("slo", Abx),
        ("JSR", Abs), ("AND", Izx), ("jam", Imp), ("rla", Izx), ("BIT", Zp), ("AND", Zp), ("ROL", Zp), ("rla", Zp),
        ("PLP", Imp), ("AND", Imm), ("ROL", Acc), ("anc", Imm), ("BIT", Abs), ("AND", Abs), ("ROL", Abs), ("rla", Abs),
        ("BMI", Rel), ("AND", Izy), ("jam", Imp), ("rla", Izy), ("nop", Zpx), ("AND", Zpx), ("ROL", Zpx), ("rla", Zpx),
        ("SEC", Imp), ("AND", Aby), ("nop", Imp), ("rla", Aby), ("nop", Abx), ("AND", Abx), ("ROL", Abx), ("rla", Abx),
        ("RTI", Imp), ("EOR", Izx), ("jam", Imp), ("sre", Izx), ("nop", Zp), ("EOR", Zp), ("LSR", Zp), ("sre", Zp),
        ("PHA", Imp), ("EOR", Imm), ("LSR", Acc), ("alr", Imm), ("JMP", Abs), ("EOR", Abs), ("LSR", Abs), ("sre", Abs),
        ("BVC", Rel), ("EOR", Izy), ("jam", Imp), ("sre", Izy), ("nop", Zpx), ("EOR", Zpx), ("LSR", Zpx), ("sre", Zpx),
        ("CLI", Imp), ("EOR", Aby), ("nop", Imp), ("sre", Aby), ("nop", Abx), ("EOR", Abx), ("LSR", Abx), ("sre", Abx),
        ("RTS", Imp), ("ADC", Izx), ("jam", Imp), ("rra", Izx), ("nop", Zp), ("ADC", Zp), ("ROR", Zp), ("rra", Zp),
        ("PLA", Imp), ("ADC", Imm), ("ROR", Acc), ("arr", Imm), ("JMP", Ind), ("ADC", Abs), ("ROR", Abs), ("rra", Abs),
        ("BVS", Rel), ("ADC", Izy), ("jam", Imp), ("rra", Izy), ("nop", Zpx), ("ADC", Zpx), ("ROR", Zpx), ("rra", Zpx),
        ("SEI", Imp), ("ADC", Aby), ("nop", Imp), ("rra", Aby), ("nop", Abx), ("ADC", Abx), ("ROR", Abx), ("rra", Abx),
        ("nop", Imm), ("STA", Izx), ("nop", Imm), ("sax", Izx), ("STY", Zp), ("STA", Zp), ("STX", Zp), ("sax", Zp),
        ("DEY", Imp), ("nop", Imm), ("TXA", Imp), ("xaa", Imm), ("STY", Abs), ("STA", Abs), ("STX", Abs), ("sax", Abs),
        ("BCC", Rel), ("STA", Izy), ("jam", Imp), ("sha", Izy), ("STY", Zpx), ("STA", Zpx), ("STX", Zpy), ("sax", Zpy),
        ("TYA", Imp), ("STA", Aby), ("TXS", Imp), ("tas", Aby), ("shy", Abx), ("STA", Abx), ("shx", Aby), ("sha", Aby),
        ("LDY", Imm), ("LDA", Izx), ("LDX", Imm), ("lax", Izx), ("LDY", Zp), ("LDA", Zp), ("LDX", Zp), ("lax", Zp),
        ("TAY", Imp), ("LDA", Imm), ("TAX", Imp), ("lax", Imm), ("LDY", Abs), ("LDA", Abs), ("LDX", Abs), ("lax", Abs),
        ("BCS", Rel), ("LDA", Izy), ("jam", Imp), ("lax", Izy), ("LDY", Zpx), ("LDA", Zpx), ("LDX", Zpy), ("lax", Zpy),
        ("CLV", Imp), ("LDA", Aby), ("TSX", Imp), ("las", Aby), ("LDY", Abx), ("LDA", Abx), ("LDX", Aby), ("lax", Aby),
        ("CPY", Imm), ("CMP", Izx), ("nop", Imm), ("dcp", Izx), ("CPY", Zp), ("CMP", Zp), ("DEC", Zp), ("dcp", Zp),
        ("INY", Imp), ("CMP", Imm), ("DEX", Imp), ("axs", Imm), ("CPY", Abs), ("CMP", Abs), ("DEC", Abs), ("dcp", Abs),
        ("BNE", Rel), ("CMP", Izy), ("jam", Imp), ("dcp", Izy), ("nop", Zpx), ("CMP", Zpx), ("DEC", Zpx), ("dcp", Zpx),
        ("CLD", Imp), ("CMP", Aby), ("nop", Imp), ("dcp", Aby), ("nop", Abx), ("CMP", Abx), ("DEC", Abx), ("dcp", Abx),
        ("CPX", Imm), ("SBC", Izx), ("nop", Imm), ("isc", Izx), ("CPX", Zp), ("SBC", Zp), ("INC", Zp), ("isc", Zp),
        ("INX", Imp), ("SBC", Imm), ("NOP", Imp), ("sbc", Imm), ("CPX", Abs), ("SBC", Abs), ("INC", Abs), ("isc", Abs),
        ("BEQ", Rel), ("SBC", Izy), ("jam", Imp), ("isc", Izy), ("nop", Zpx), ("SBC", Zpx), ("INC", Zpx), ("isc", Zpx),
        ("SED", Imp), ("SBC", Aby), ("nop", Imp), ("isc", Aby), ("nop", Abx), ("SBC", Abx), ("INC", Abx), ("isc", Abx),
    ];
    T[op as usize]
}

/// `LDA $0200,X` for an opcode at CPU address `pc` and its operand bytes. `name` turns a
/// CPU address into a label when one is known.
pub fn disasm_6502(pc: u16, op: u8, operands: &[u8], name: &dyn Fn(u16) -> Option<String>) -> String {
    let (m, mode) = op_6502(op);
    let b = |i: usize| operands.get(i).copied().unwrap_or(0);
    let w = u16::from_le_bytes([b(0), b(1)]);
    let lab = |a: u16, digits: usize| name(a).unwrap_or_else(|| if digits == 2 { format!("${a:02X}") } else { format!("${a:04X}") });
    match mode {
        Mode::Imp => m.to_string(),
        Mode::Acc => format!("{m} A"),
        Mode::Imm => format!("{m} #${:02X}", b(0)),
        Mode::Zp => format!("{m} {}", lab(b(0) as u16, 2)),
        Mode::Zpx => format!("{m} {},X", lab(b(0) as u16, 2)),
        Mode::Zpy => format!("{m} {},Y", lab(b(0) as u16, 2)),
        Mode::Abs => format!("{m} {}", lab(w, 4)),
        Mode::Abx => format!("{m} {},X", lab(w, 4)),
        Mode::Aby => format!("{m} {},Y", lab(w, 4)),
        Mode::Ind => format!("{m} ({})", lab(w, 4)),
        Mode::Izx => format!("{m} (${:02X},X)", b(0)),
        Mode::Izy => format!("{m} (${:02X}),Y", b(0)),
        Mode::Rel => format!("{m} {}", lab(pc.wrapping_add(2).wrapping_add(b(0) as i8 as u16), 4)),
    }
}

/// Instructions in a trace, each with the data reads and writes it made.
pub struct Step<'a> {
    pub exec: &'a TraceEvent,
    pub accesses: Vec<&'a TraceEvent>,
}

pub fn group(events: &[TraceEvent]) -> Vec<Step<'_>> {
    let mut out: Vec<Step> = vec![];
    for e in events {
        if e.kind == TRACE_EXEC {
            out.push(Step { exec: e, accesses: vec![] });
        } else if let Some(last) = out.last_mut() {
            last.accesses.push(e);
        }
    }
    out
}

/// One line per instruction: `$C123 LDA $0200,X  A:00 X:01 Y:00 S:FD P:24  r$0201=05 w$0300=05`.
pub fn line(s: &Step, name: &dyn Fn(u16) -> Option<String>) -> String {
    let e = s.exec;
    let mut t = format!(
        "${:04X} {:<18} A:{:02X} X:{:02X} Y:{:02X} S:{:02X} P:{:02X}",
        e.addr,
        disasm_6502(e.addr, e.value, &e.operands[..e.operand_count as usize], name),
        e.a,
        e.x,
        e.y,
        e.sp,
        e.p
    );
    for a in &s.accesses {
        let k = match a.kind {
            TRACE_READ => 'r',
            TRACE_WRITE => 'w',
            _ => '?',
        };
        t.push_str(&format!(" {k}${:04X}={:02X}", a.addr, a.value));
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disassembles() {
        let none = |_: u16| None;
        assert_eq!(disasm_6502(0xC000, 0xA9, &[0x05], &none), "LDA #$05");
        assert_eq!(disasm_6502(0xC000, 0xBD, &[0x00, 0x02], &none), "LDA $0200,X");
        assert_eq!(disasm_6502(0xC000, 0xD0, &[0xFE], &none), "BNE $C000");
        assert_eq!(disasm_6502(0xC000, 0x20, &[0x34, 0x12], &|a| (a == 0x1234).then(|| "f".into())), "JSR f");
        assert_eq!(disasm_6502(0xC000, 0xB1, &[0x10], &none), "LDA ($10),Y");
        assert_eq!(disasm_6502(0xC000, 0x0A, &[], &none), "ASL A");
        assert_eq!(std::mem::size_of::<TraceEvent>(), 40);
    }
}
