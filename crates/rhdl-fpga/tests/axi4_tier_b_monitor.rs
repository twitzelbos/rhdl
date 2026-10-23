//! Tier A and Tier B must agree, on every transcript, on every output.
//!
//! `axi4-architecture.md` §7 asks for two protocol monitors and gives
//! the reason: Tier A is an RHDL widget checking RHDL widgets, so a
//! misreading of the AMBA specification shared between the monitor and
//! the thing it monitors passes silently. Tier B is a hand-written
//! plain-Verilog monitor, in `verilog/axi4/axi4_write_monitor.v`, written
//! from the spec and deliberately implemented by other means — lanes
//! walked rather than masks shifted, addresses accumulated rather than
//! recomputed, pages compared by last byte rather than by last beat.
//!
//! This test is what makes the second monitor worth having. For every
//! transcript in [`Violation::all`] it requires:
//!
//! 1. Tier A reports the injected fault,
//! 2. Tier B reports the injected fault, and
//! 3. the two agree on **all three** outputs, including the fault
//!    tally and the count of unchecked beats.
//!
//! Clause 3 is the one with teeth. Agreement on the verdict alone would
//! be satisfied by two monitors that both detect the fault for
//! different reasons at different times; agreement on the counts means
//! they saw the same events on the same cycles. It has already earned
//! its place: the Verilog monitor's first draft compared only `BID`
//! across a stalled response beat, not `BRESP`, so it was blind to
//! `BPayloadChanged` — a hole in the independent oracle that only a
//! disagreement with the dependent one could reveal.

mod common;

use common::{code_of, tier_b_write};
use rhdl::prelude::*;
use rhdl_fpga::axi4::check::monitor::Axi4WriteMonitor;
use rhdl_fpga::axi4::check::read_monitor::Axi4ReadMonitor;
use rhdl_fpga::axi4::check::traffic::{
    ReadCycle, ReadViolation, Violation, read_transcript, write_transcript,
};
use rhdl_fpga::axi4::types::{encode_burst, encode_response};

const A: usize = 32;
const DW: usize = 32;
const STRB: usize = 4;
const ID: usize = 4;
/// One outstanding-count slot per possible ID: `1 << ID`.
const NIDS: usize = 16;
const BUS_BYTES: u128 = 4;

/// Tier A's verdict: `(fault code, fault tally, unchecked beats)`.
fn tier_a(v: Violation) -> (u32, u128, u128) {
    let uut = Axi4WriteMonitor::<A, DW, STRB, ID, NIDS>::default();
    let cycles = write_transcript::<A, DW, STRB, ID>(BUS_BYTES, v);
    let input = cycles.into_iter().with_reset(1).clock_pos_edge(100);
    let out = uut
        .run(input)
        .synchronous_sample()
        .map(|x| x.output)
        .collect::<Vec<_>>();
    let last = *out.last().unwrap();
    (
        code_of(last.fault),
        last.faults.raw(),
        last.unchecked_beats.raw(),
    )
}

/// The cross-check. Both monitors, every transcript, all three outputs.
#[test]
fn tier_a_and_tier_b_agree_on_every_transcript() {
    // Tier B *is* the ground truth here, so a missing toolchain must
    // stop the run with a message rather than quietly prove nothing.
    rhdl::vlog::toolchain::require_iverilog();
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("axi4_tier_b");
    std::fs::create_dir_all(&dir).unwrap();

    let mut wrong: Vec<String> = Vec::new();
    for v in Violation::all() {
        let want = code_of(v.expected());
        let a = tier_a(v);
        let b = tier_b_write(
            &write_transcript::<A, DW, STRB, ID>(BUS_BYTES, v),
            &dir,
            &format!("{v:?}"),
        );
        if a.0 != want {
            wrong.push(format!("{v:?}: tier A said {}, expected {want}", a.0));
        }
        if b.0 != want {
            wrong.push(format!("{v:?}: tier B said {}, expected {want}", b.0));
        }
        if a != b {
            wrong.push(format!(
                "{v:?}: monitors disagree -- A {a:?} vs B {b:?} \
                 (fault, tally, unchecked)"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Tier B's read-channel verdict.
fn tier_b_read(v: ReadViolation, dir: &std::path::Path) -> (u32, u128, u128) {
    let cycles = read_transcript::<A, DW, ID>(BUS_BYTES, v);
    let tb = dir.join(format!("tb_read_{v:?}.v"));
    let exe = dir.join(format!("tb_read_{v:?}.vvp"));
    std::fs::write(&tb, read_testbench(&cycles)).unwrap();

    let monitor = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("verilog")
        .join("axi4")
        .join("axi4_read_monitor.v");

    let out = std::process::Command::new("iverilog")
        .arg("-g2001")
        .arg("-o")
        .arg(&exe)
        .arg(&tb)
        .arg(&monitor)
        .output()
        .expect("iverilog should be runnable");
    assert!(
        out.status.success(),
        "iverilog failed for {v:?}:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = std::process::Command::new("vvp")
        .arg(&exe)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&run.stdout);
    let line = text
        .lines()
        .find(|l| l.starts_with("VERDICT"))
        .unwrap_or_else(|| panic!("no verdict from Tier B for {v:?}; output was:\n{text}"));
    let n: Vec<u128> = line
        .split_whitespace()
        .skip(1)
        .map(|t| t.parse().unwrap())
        .collect();
    (n[0] as u32, n[1], n[2])
}

/// Emit a testbench driving a read transcript into the hand-written
/// read monitor.
fn read_testbench(cycles: &[ReadCycle<A, DW, ID>]) -> String {
    let mut s = String::new();
    s.push_str(
        "`timescale 1ns/1ps\nmodule tb;\n\
         reg clk = 0, rstn = 0;\n\
         reg ar_valid, ar_ready; reg [3:0] ar_id; reg [31:0] ar_addr;\n\
         reg [7:0] ar_len; reg [2:0] ar_size; reg [1:0] ar_burst;\n\
         reg r_valid, r_ready; reg [3:0] r_id; reg [31:0] r_data;\n\
         reg [1:0] r_resp; reg r_last;\n\
         wire [4:0] fault; wire [15:0] faults; wire [15:0] unchecked;\n\
         axi4_read_monitor m(.clk(clk), .rstn(rstn),\n\
           .ar_valid(ar_valid), .ar_ready(ar_ready), .ar_id(ar_id),\n\
           .ar_addr(ar_addr), .ar_len(ar_len), .ar_size(ar_size),\n\
           .ar_burst(ar_burst),\n\
           .r_valid(r_valid), .r_ready(r_ready), .r_id(r_id),\n\
           .r_data(r_data), .r_resp(r_resp), .r_last(r_last),\n\
           .fault(fault), .faults(faults), .unchecked(unchecked));\n\
         always #5 clk = ~clk;\n\
         initial begin\n\
         ar_valid=0; ar_ready=0; ar_id=0; ar_addr=0; ar_len=0; ar_size=0; ar_burst=0;\n\
         r_valid=0; r_ready=0; r_id=0; r_data=0; r_resp=0; r_last=0;\n\
         rstn = 0;\n\
         @(negedge clk);\n\
         rstn = 1;\n",
    );
    for c in cycles {
        s.push_str(&format!(
            "ar_valid={}; ar_ready={}; ar_id={}; ar_addr={}; ar_len={}; ar_size={}; ar_burst={};\n\
             r_valid={}; r_ready={}; r_id={}; r_data={}; r_resp={}; r_last={};\n\
             @(negedge clk);\n",
            c.ar_valid as u8,
            c.ar_ready as u8,
            c.ar.id.raw(),
            c.ar.addr.raw(),
            c.ar.len.raw(),
            c.ar.size.raw(),
            encode_burst(c.ar.burst).raw(),
            c.r_valid as u8,
            c.r_ready as u8,
            c.r.id.raw(),
            c.r.data.raw(),
            encode_response(c.r.resp).raw(),
            c.r.last as u8,
        ));
    }
    s.push_str(
        "@(posedge clk); #1;\n\
         $display(\"VERDICT %0d %0d %0d\", fault, faults, unchecked);\n\
         $finish;\n end\nendmodule\n",
    );
    s
}

/// Tier A's read-channel verdict.
fn tier_a_read(v: ReadViolation) -> (u32, u128, u128) {
    let uut = Axi4ReadMonitor::<A, DW, STRB, ID, NIDS>::default();
    let cycles = read_transcript::<A, DW, ID>(BUS_BYTES, v);
    let input = cycles.into_iter().with_reset(1).clock_pos_edge(100);
    let out = uut
        .run(input)
        .synchronous_sample()
        .map(|x| x.output)
        .collect::<Vec<_>>();
    let last = *out.last().unwrap();
    (
        code_of(last.fault),
        last.faults.raw(),
        last.unchecked_beats.raw(),
    )
}

/// The cross-check, read channels. Both monitors, every transcript, all
/// three outputs.
#[test]
fn tier_a_and_tier_b_agree_on_every_read_transcript() {
    rhdl::vlog::toolchain::require_iverilog();
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("axi4_tier_b");
    std::fs::create_dir_all(&dir).unwrap();

    let mut wrong: Vec<String> = Vec::new();
    for v in ReadViolation::all() {
        let want = code_of(v.expected());
        let a = tier_a_read(v);
        let b = tier_b_read(v, &dir);
        if a.0 != want {
            wrong.push(format!("{v:?}: tier A said {}, expected {want}", a.0));
        }
        if b.0 != want {
            wrong.push(format!("{v:?}: tier B said {}, expected {want}", b.0));
        }
        if a != b {
            wrong.push(format!(
                "{v:?}: monitors disagree -- A {a:?} vs B {b:?} \
                 (fault, tally, unchecked)"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
