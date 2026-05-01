//! R-register file tests.

use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;
use rhdl_alto::regfile::{In, Out, RegFile};

fn b16(v: u16) -> Bits<16> { bits::<16>(v as u128) }
fn b5(v: u8) -> Bits<5> { bits::<5>(v as u128) }

fn run(uut: RegFile, inputs: Vec<In>, max_cycles: usize) -> Vec<u16> {
    let mut reset_remaining = 2;
    let mut total = 0;
    let mut cursor = inputs.into_iter();
    let mut last_input = In::default();
    let mut readings: Vec<u16> = Vec::new();
    uut.run_fn(
        |out: Out| {
            if reset_remaining > 0 { reset_remaining -= 1; return Some(ResetOrData::Reset); }
            if total >= max_cycles { return None; }
            total += 1;
            readings.push(out.rdata.raw() as u16);
            match cursor.next() {
                Some(i) => { last_input = i; Some(ResetOrData::Data(i)) }
                None    => Some(ResetOrData::Data(last_input)),
            }
        },
        100,
    ).for_each(drop);
    readings
}

#[test]
fn write_then_read_round_trip() {
    let uut = RegFile::default();
    let inputs = vec![
        In { raddr: b5(0), waddr: b5(5), wdata: b16(0xCAFE), wen: true },
        In { raddr: b5(5), waddr: b5(0), wdata: b16(0),      wen: false },
        In { raddr: b5(5), waddr: b5(0), wdata: b16(0),      wen: false },
    ];
    let r = run(uut, inputs, 5);
    // After the write at cycle 0 commits, cycle 1's read of R5 returns 0xCAFE.
    assert_eq!(r[2], 0xCAFE);
}

#[test]
fn r_zero_is_writable_unlike_rv32i_x0() {
    // The Alto's R0 is a normal R/W register (no hardwired zero).
    let uut = RegFile::default();
    let inputs = vec![
        In { raddr: b5(0), waddr: b5(0), wdata: b16(0xABCD), wen: true },
        In { raddr: b5(0), waddr: b5(0), wdata: b16(0),      wen: false },
        In { raddr: b5(0), waddr: b5(0), wdata: b16(0),      wen: false },
    ];
    let r = run(uut, inputs, 5);
    assert_eq!(r[2], 0xABCD, "Alto R0 should retain a written value");
}

#[test]
fn writes_are_independent() {
    let uut = RegFile::default();
    let inputs = vec![
        In { raddr: b5(0),  waddr: b5(7),  wdata: b16(0x77), wen: true },
        In { raddr: b5(0),  waddr: b5(15), wdata: b16(0x15), wen: true },
        In { raddr: b5(7),  waddr: b5(0),  wdata: b16(0),    wen: false },
        In { raddr: b5(15), waddr: b5(0),  wdata: b16(0),    wen: false },
        In { raddr: b5(15), waddr: b5(0),  wdata: b16(0),    wen: false },
    ];
    let r = run(uut, inputs, 7);
    assert_eq!(r[3], 0x77, "R7 retained 0x77");
    assert_eq!(r[4], 0x15, "R15 retained 0x15");
}

#[test]
fn iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = RegFile::default();
    let inputs: Vec<In> = (0..8).map(|i| In {
        raddr: b5(i as u8 & 0x1F),
        waddr: b5((i as u8 + 1) & 0x1F),
        wdata: b16((i * 0x100) as u16),
        wen: i % 2 == 0,
    }).collect();
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
