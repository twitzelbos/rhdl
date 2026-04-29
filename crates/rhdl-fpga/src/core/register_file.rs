//! Generic memory-mapped register file
//!
//! `N` storage registers of arbitrary `T: Digital + Default`,
//! addressed by an `W`-bit index, with **combinational read** and
//! **registered write** semantics.  Bus-agnostic: an AXI4-Lite,
//! Wishbone, or APB adapter wraps this widget by translating its
//! protocol's `(read_addr, read_enable)` and
//! `(write_addr, write_data, write_enable)` to the simple inputs
//! below.
//!
//! The existing [super::super::axi4lite::register] widgets
//! (single, bank, rom) couple register storage to the AXI4-Lite
//! protocol.  This widget is a strict generalization — same
//! register semantics, no protocol assumptions.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +--+RegisterFile+--+
     |                  |
B<W> |                  | T
+--->| read_addr  read  +--->
B<W> |                  | [T;N]
+--->| write_addr regs  +--->
T    |                  |
+--->| write_data       |
bool |                  |
+--->| write_en         |
     +------------------+
")]
//!
//!# Internals
//!
//! `N` parallel [DFF]s, one per register, plus a one-hot decoder on
//! `write_addr` (a single register is loaded when `write_enable` is
//! high) and a multiplexer on `read_addr` (`read_data` is the
//! same-cycle combinational selection).  All registers are also
//! exposed verbatim as `registers[N]` for client-side use without
//! going through the address decoder.
//!
//!# Behavior
//!
//! - **Write:** when `write_enable == true` and `write_addr == k`
//!   for some `k < N`, register `k` accepts `write_data` on the
//!   next clock edge.  All other registers hold.
//! - **Read:** `read_data` is `registers[read_addr]` combinationally
//!   when `read_addr < N`.  `read_addr >= N` yields `T::dont_care()`.
//!   `read_enable` is supplied for adapter convenience but does not
//!   affect the data path — the adapter is responsible for gating
//!   `read_valid` etc.
//! - **Reset:** all registers reset to `T::default()`.  For
//!   per-register reset values, build the file with
//!   [Self::with_reset_values].
//!
//!# Concurrent read+write to the same address
//!
//! Same-cycle read of an address being written returns the **old**
//! value (the new value is not yet latched).  This matches the
//! standard "registered write, combinational read" memory model.
//!
//!# Parameters
//!
//! - `T` — data type held in each register (must be `Digital`)
//! - `N` — number of registers
//! - `W` — width of the address index, satisfying `2^W >= N`
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/register_file.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/register_file.md")]

use rhdl::prelude::*;

use super::dff;

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// `N`-register file holding values of type `T`.
pub struct RegisterFile<T: Digital + Default, const N: usize, const W: usize>
where
    rhdl::bits::W<W>: BitWidth,
{
    regs: [dff::DFF<T>; N],
}

impl<T: Digital + Default, const N: usize, const W: usize> Default for RegisterFile<T, N, W>
where
    rhdl::bits::W<W>: BitWidth,
{
    fn default() -> Self {
        Self {
            regs: array_init::array_init(|_| dff::DFF::default()),
        }
    }
}

impl<T: Digital + Default, const N: usize, const W: usize> RegisterFile<T, N, W>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// Create a register file with per-register reset values.
    pub fn with_reset_values(values: [T; N]) -> Self {
        Self {
            regs: array_init::array_init(|i| dff::DFF::new(values[i])),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [RegisterFile].
pub struct In<T: Digital, const W: usize>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The address to read combinationally.
    pub read_addr: Bits<W>,
    /// (Optional) read-strobe — passed through to `read_valid` for
    /// adapter convenience; does not affect the data path.
    pub read_enable: bool,
    /// The address to write on the next clock edge (when `write_enable` is high).
    pub write_addr: Bits<W>,
    /// The data to write.
    pub write_data: T,
    /// Strobe to perform the write this cycle.
    pub write_enable: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from the [RegisterFile].
pub struct Out<T: Digital, const N: usize> {
    /// Combinational read of the register at `read_addr`.
    pub read_data: T,
    /// Echoes the input `read_enable`, registered one cycle later
    /// for adapter pipelining.  Returns the *previous* cycle's
    /// `read_enable`.  Adapters that want the *current* cycle's
    /// strobe should use the input directly.
    pub read_valid: bool,
    /// Live view of every register, in index order.  Always present.
    pub registers: [T; N],
}

impl<T: Digital + Default, const N: usize, const W: usize> SynchronousIO for RegisterFile<T, N, W>
where
    rhdl::bits::W<W>: BitWidth,
{
    type I = In<T, W>;
    type O = Out<T, N>;
    type Kernel = register_file<T, N, W>;
}

#[kernel]
/// Kernel for [RegisterFile].
pub fn register_file<T: Digital, const N: usize, const W: usize>(
    cr: ClockReset,
    i: In<T, W>,
    q: Q<T, N, W>,
) -> (Out<T, N>, D<T, N, W>)
where
    T: Default,
    rhdl::bits::W<W>: BitWidth,
{
    let mut d = D::<T, N, W>::dont_care();
    // Default: every register holds its current value.
    for k in 0..N {
        d.regs[k] = q.regs[k];
    }
    // Apply write.  At most one register's `d` overrides the hold.
    for k in 0..N {
        if i.write_enable && i.write_addr == bits(k as u128) {
            d.regs[k] = i.write_data;
        }
    }
    // Combinational read via runtime array indexing.  RHDL lowers
    // this to an N-input mux on `read_addr`.  When `read_addr >= N`
    // the result is implementation-defined.
    let mut o = Out::<T, N>::dont_care();
    o.read_data = q.regs[i.read_addr];
    o.read_valid = i.read_enable;
    for k in 0..N {
        o.registers[k] = q.regs[k];
    }
    if cr.reset.any() {
        for k in 0..N {
            d.regs[k] = T::default();
        }
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    fn idle_in() -> In<Bits<8>, 2> {
        In {
            read_addr: bits(0),
            read_enable: false,
            write_addr: bits(0),
            write_data: bits(0),
            write_enable: false,
        }
    }

    // Tier 1 — direct kernel unit tests

    #[test]
    fn test_default_reset_zeros_all_registers() {
        let cr = clock_reset(clock(true), reset(true));
        let q = Q::<Bits<8>, 4, 2> {
            regs: [bits(0xAA), bits(0xBB), bits(0xCC), bits(0xDD)],
        };
        let (_o, d) = register_file::<Bits<8>, 4, 2>(cr, idle_in(), q);
        for k in 0..4 {
            assert_eq!(d.regs[k], bits(0));
        }
    }

    #[test]
    fn test_write_takes_effect_on_addressed_register_only() {
        let cr = ClockReset::dont_care();
        let q = Q::<Bits<8>, 4, 2> {
            regs: [bits(0x11), bits(0x22), bits(0x33), bits(0x44)],
        };
        let mut i = idle_in();
        i.write_addr = bits(2);
        i.write_data = bits(0xCC);
        i.write_enable = true;
        let (_o, d) = register_file::<Bits<8>, 4, 2>(cr, i, q);
        assert_eq!(d.regs[0], bits(0x11));
        assert_eq!(d.regs[1], bits(0x22));
        assert_eq!(d.regs[2], bits(0xCC));
        assert_eq!(d.regs[3], bits(0x44));
    }

    #[test]
    fn test_write_disabled_holds_all_registers() {
        let cr = ClockReset::dont_care();
        let q = Q::<Bits<8>, 4, 2> {
            regs: [bits(0x11), bits(0x22), bits(0x33), bits(0x44)],
        };
        let mut i = idle_in();
        i.write_addr = bits(2);
        i.write_data = bits(0xCC);
        i.write_enable = false;
        let (_o, d) = register_file::<Bits<8>, 4, 2>(cr, i, q);
        assert_eq!(d.regs[0], bits(0x11));
        assert_eq!(d.regs[2], bits(0x33));
    }

    #[test]
    fn test_read_returns_addressed_register_combinationally() {
        let cr = ClockReset::dont_care();
        let q = Q::<Bits<8>, 4, 2> {
            regs: [bits(0x11), bits(0x22), bits(0x33), bits(0x44)],
        };
        for addr in 0u128..4 {
            let mut i = idle_in();
            i.read_addr = bits(addr);
            i.read_enable = true;
            let (o, _d) = register_file::<Bits<8>, 4, 2>(cr, i, q);
            assert_eq!(o.read_data.raw(), 0x11 + addr * 0x11);
            assert!(o.read_valid);
        }
    }

    #[test]
    fn test_concurrent_read_write_same_addr_returns_old_value() {
        let cr = ClockReset::dont_care();
        let q = Q::<Bits<8>, 4, 2> {
            regs: [bits(0x11), bits(0x22), bits(0x33), bits(0x44)],
        };
        let mut i = idle_in();
        i.read_addr = bits(2);
        i.read_enable = true;
        i.write_addr = bits(2);
        i.write_data = bits(0xCC);
        i.write_enable = true;
        let (o, d) = register_file::<Bits<8>, 4, 2>(cr, i, q);
        // Read returns OLD value (not the just-being-written one).
        assert_eq!(o.read_data, bits(0x33));
        // But the write IS captured for next cycle.
        assert_eq!(d.regs[2], bits(0xCC));
    }

    #[test]
    fn test_registers_output_exposes_all_regs() {
        let cr = ClockReset::dont_care();
        let q = Q::<Bits<8>, 4, 2> {
            regs: [bits(0x11), bits(0x22), bits(0x33), bits(0x44)],
        };
        let (o, _d) = register_file::<Bits<8>, 4, 2>(cr, idle_in(), q);
        assert_eq!(o.registers[0], bits(0x11));
        assert_eq!(o.registers[1], bits(0x22));
        assert_eq!(o.registers[2], bits(0x33));
        assert_eq!(o.registers[3], bits(0x44));
    }

    // Tier 2 — iterator simulation

    /// Write each address sequentially, then read each back; verify
    /// the values land in the right register.
    #[test]
    fn test_write_then_read_sequence() -> miette::Result<()> {
        let mut stream_in: Vec<In<Bits<8>, 2>> = Vec::new();
        // Write phase: addr 0 ← 0xA0, addr 1 ← 0xA1, addr 2 ← 0xA2, addr 3 ← 0xA3.
        for addr in 0u128..4 {
            stream_in.push(In {
                read_addr: bits(0),
                read_enable: false,
                write_addr: bits(addr),
                write_data: bits(0xA0 + addr),
                write_enable: true,
            });
        }
        // Read phase: read each addr.
        for addr in 0u128..4 {
            stream_in.push(In {
                read_addr: bits(addr),
                read_enable: true,
                write_addr: bits(0),
                write_data: bits(0),
                write_enable: false,
            });
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let uut = RegisterFile::<Bits<8>, 4, 2>::default();
        let outputs = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output.read_data.raw())
            .collect::<Vec<_>>();
        // The last 4 samples should be the read responses.
        // Combinational read means the response appears the same cycle
        // as the read input, but writes have a 1-cycle latency.  So
        // by the read phase (samples 4..8), all writes are committed.
        let read_responses = &outputs[4..8];
        assert_eq!(read_responses, &[0xA0, 0xA1, 0xA2, 0xA3]);
        Ok(())
    }

    // Tier 3 — HDL emission length sanity check
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = RegisterFile::<Bits<8>, 4, 2>::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let expect = expect!["7270"];
        expect.assert_eq(&hdl.len().to_string());
        Ok(())
    }

    // Tier 4 — iverilog round-trip
    #[test]
    fn test_register_file_hdl_works() -> miette::Result<()> {
        let uut = RegisterFile::<Bits<8>, 4, 2>::default();
        let mut stream_in: Vec<In<Bits<8>, 2>> = Vec::new();
        for addr in 0u128..4 {
            stream_in.push(In {
                read_addr: bits(0),
                read_enable: false,
                write_addr: bits(addr),
                write_data: bits(0xA0 + addr),
                write_enable: true,
            });
        }
        for addr in 0u128..4 {
            stream_in.push(In {
                read_addr: bits(addr),
                read_enable: true,
                write_addr: bits(0),
                write_data: bits(0),
                write_enable: false,
            });
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest
    #[test]
    fn test_register_file_trace() -> miette::Result<()> {
        let uut = RegisterFile::<Bits<8>, 4, 2>::default();
        let mut stream_in: Vec<In<Bits<8>, 2>> = Vec::new();
        for addr in 0u128..4 {
            stream_in.push(In {
                read_addr: bits(0),
                read_enable: false,
                write_addr: bits(addr),
                write_data: bits(0xA0 + addr),
                write_enable: true,
            });
        }
        for addr in 0u128..4 {
            stream_in.push(In {
                read_addr: bits(addr),
                read_enable: true,
                write_addr: bits(0),
                write_data: bits(0),
                write_enable: false,
            });
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("register_file");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["29f8665620f7756cf44a61c5986aa91c66af4a9dee392c7e1f5026ae8c499738"];
        let digest = vcd.dump_to_file(root.join("register_file.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
