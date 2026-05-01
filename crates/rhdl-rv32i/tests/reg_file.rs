//! Register-file tests.
//!
//! Verify x0 hardwired to zero, write-then-read end-to-end, and
//! write-disable.

use rhdl::prelude::*;
use rhdl_rv32i::reg_file::*;

fn b32(x: u32) -> Bits<32> {
    bits::<32>(x as u128)
}

fn r5(x: u8) -> Bits<5> {
    bits::<5>(x as u128)
}

#[test]
fn reads_zero_from_x0_after_reset() {
    let uut = RegFile::default();
    let inputs = vec![
        // Reset cycle then a read of x0.
        In {
            raddr1: r5(0),
            raddr2: r5(0),
            waddr: r5(0),
            wdata: b32(0),
            wen: false,
        },
    ];
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let outputs: Vec<Out> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    let last = outputs.last().expect("at least one sample");
    assert_eq!(last.rdata1, b32(0));
    assert_eq!(last.rdata2, b32(0));
}

#[test]
fn write_then_read_roundtrips() {
    let uut = RegFile::default();
    let inputs = vec![
        // Cycle 0: write 0xCAFE_BABE to x5.
        In {
            raddr1: r5(5),
            raddr2: r5(0),
            waddr: r5(5),
            wdata: b32(0xCAFE_BABE),
            wen: true,
        },
        // Cycle 1: read x5.
        In {
            raddr1: r5(5),
            raddr2: r5(0),
            waddr: r5(0),
            wdata: b32(0),
            wen: false,
        },
        // Cycle 2: still reading x5.
        In {
            raddr1: r5(5),
            raddr2: r5(0),
            waddr: r5(0),
            wdata: b32(0),
            wen: false,
        },
    ];
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let outputs: Vec<Out> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    // The last cycle reads x5 — should be 0xCAFE_BABE.
    let last = outputs.last().expect("samples");
    assert_eq!(last.rdata1, b32(0xCAFE_BABE), "outputs: {outputs:?}");
}

#[test]
fn write_to_x0_is_silently_dropped() {
    let uut = RegFile::default();
    let inputs = vec![
        // Try to write 0xDEAD_BEEF to x0.
        In {
            raddr1: r5(0),
            raddr2: r5(0),
            waddr: r5(0),
            wdata: b32(0xDEAD_BEEF),
            wen: true,
        },
        // Read x0 — should still be zero.
        In {
            raddr1: r5(0),
            raddr2: r5(0),
            waddr: r5(0),
            wdata: b32(0),
            wen: false,
        },
    ];
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let outputs: Vec<Out> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    for o in &outputs {
        assert_eq!(o.rdata1, b32(0));
        assert_eq!(o.rdata2, b32(0));
    }
}

#[test]
fn two_read_ports_serve_different_addresses() {
    let uut = RegFile::default();
    let inputs = vec![
        // Cycle 0: write 0x111 to x5.
        In {
            raddr1: r5(0),
            raddr2: r5(0),
            waddr: r5(5),
            wdata: b32(0x111),
            wen: true,
        },
        // Cycle 1: write 0x222 to x6.
        In {
            raddr1: r5(0),
            raddr2: r5(0),
            waddr: r5(6),
            wdata: b32(0x222),
            wen: true,
        },
        // Cycle 2: read x5 on port 1 and x6 on port 2.
        In {
            raddr1: r5(5),
            raddr2: r5(6),
            waddr: r5(0),
            wdata: b32(0),
            wen: false,
        },
    ];
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let outputs: Vec<Out> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    let last = outputs.last().expect("samples");
    assert_eq!(last.rdata1, b32(0x111), "rdata1 (x5)");
    assert_eq!(last.rdata2, b32(0x222), "rdata2 (x6)");
}

#[test]
fn iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = RegFile::default();
    let inputs = vec![
        In {
            raddr1: r5(1),
            raddr2: r5(2),
            waddr: r5(1),
            wdata: b32(0xAA),
            wen: true,
        },
        In {
            raddr1: r5(1),
            raddr2: r5(2),
            waddr: r5(2),
            wdata: b32(0xBB),
            wen: true,
        },
        In {
            raddr1: r5(1),
            raddr2: r5(2),
            waddr: r5(0),
            wdata: b32(0),
            wen: false,
        },
    ];
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
