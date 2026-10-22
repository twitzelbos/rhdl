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

use rhdl::prelude::*;
use rhdl_fpga::axi4::check::monitor::Axi4WriteMonitor;
use rhdl_fpga::axi4::check::read_monitor::Axi4ReadMonitor;
use rhdl_fpga::axi4::check::rules::Fault;
use rhdl_fpga::axi4::check::traffic::{
    ReadCycle, ReadViolation, Violation, WriteCycle, read_transcript, write_transcript,
};
use rhdl_fpga::axi4::types::{encode_burst, encode_response};
use std::io::Write;

const A: usize = 32;
const DW: usize = 32;
const STRB: usize = 4;
const ID: usize = 4;
const BUS_BYTES: u128 = 4;

/// The fault-code contract shared with `axi4_write_monitor.v`.
///
/// Written out rather than taken from the derived `Digital`
/// discriminant, so that a reordering of the Rust enum cannot silently
/// re-point the Verilog's `\`define`s at different faults.
fn code_of(f: Fault) -> u32 {
    match f {
        Fault::None => 0,
        Fault::AwValidDropped => 1,
        Fault::AwPayloadChanged => 2,
        Fault::WValidDropped => 3,
        Fault::WPayloadChanged => 4,
        Fault::BValidDropped => 5,
        Fault::BPayloadChanged => 6,
        Fault::WLastEarly => 7,
        Fault::WLastMissing => 8,
        Fault::BrespUnexpected => 9,
        Fault::IncrCrosses4K => 10,
        Fault::WrapBadLength => 11,
        Fault::WrapUnaligned => 12,
        Fault::SizeExceedsBus => 13,
        Fault::StrobeOutsideLanes => 14,
        // The read-channel codes start at 15 rather than following the
        // enum's declaration order. That is the point of writing this
        // mapping out by hand: adding the read variants inserted them
        // into the MIDDLE of the shared `Fault` enum, and a mapping
        // derived from the discriminant would have silently re-pointed
        // the write monitor's existing `\`define`s at different faults.
        // Assigning above the write codes instead meant
        // `axi4_write_monitor.v` never had to change.
        Fault::ArValidDropped => 15,
        Fault::ArPayloadChanged => 16,
        Fault::RValidDropped => 17,
        Fault::RPayloadChanged => 18,
        Fault::RLastEarly => 19,
        Fault::RLastMissing => 20,
        Fault::RBeatUnexpected => 21,
        Fault::RIdMismatch => 22,
    }
}

/// Tier A's verdict: `(fault code, fault tally, unchecked beats)`.
fn tier_a(v: Violation) -> (u32, u128, u128) {
    let uut = Axi4WriteMonitor::<A, DW, STRB, ID>::default();
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

/// Emit a self-contained Verilog testbench driving the transcript into
/// the hand-written monitor.
///
/// Signals are applied on the falling edge so that they are stable
/// across the rising edge the monitor samples on, and `rstn` is held
/// low for one sampled edge to match Tier A's `with_reset(1)`.
fn write_testbench(cycles: &[WriteCycle<A, DW, STRB, ID>]) -> String {
    let mut s = String::new();
    s.push_str(
        "`timescale 1ns/1ps\nmodule tb;\n\
         reg clk = 0, rstn = 0;\n\
         reg aw_valid, aw_ready; reg [3:0] aw_id; reg [31:0] aw_addr;\n\
         reg [7:0] aw_len; reg [2:0] aw_size; reg [1:0] aw_burst;\n\
         reg w_valid, w_ready; reg [31:0] w_data; reg [3:0] w_strb; reg w_last;\n\
         reg b_valid, b_ready; reg [3:0] b_id; reg [1:0] b_resp;\n\
         wire [4:0] fault; wire [15:0] faults; wire [15:0] unchecked;\n\
         axi4_write_monitor m(.clk(clk), .rstn(rstn),\n\
           .aw_valid(aw_valid), .aw_ready(aw_ready), .aw_id(aw_id),\n\
           .aw_addr(aw_addr), .aw_len(aw_len), .aw_size(aw_size),\n\
           .aw_burst(aw_burst),\n\
           .w_valid(w_valid), .w_ready(w_ready), .w_data(w_data),\n\
           .w_strb(w_strb), .w_last(w_last),\n\
           .b_valid(b_valid), .b_ready(b_ready), .b_id(b_id), .b_resp(b_resp),\n\
           .fault(fault), .faults(faults), .unchecked(unchecked));\n\
         always #5 clk = ~clk;\n\
         initial begin\n\
         aw_valid=0; aw_ready=0; aw_id=0; aw_addr=0; aw_len=0; aw_size=0; aw_burst=0;\n\
         w_valid=0; w_ready=0; w_data=0; w_strb=0; w_last=0;\n\
         b_valid=0; b_ready=0; b_id=0; b_resp=0;\n\
         rstn = 0;\n\
         @(negedge clk);\n\
         rstn = 1;\n",
    );
    for c in cycles {
        s.push_str(&format!(
            "aw_valid={}; aw_ready={}; aw_id={}; aw_addr={}; aw_len={}; aw_size={}; aw_burst={};\n\
             w_valid={}; w_ready={}; w_data={}; w_strb={}; w_last={};\n\
             b_valid={}; b_ready={}; b_id={}; b_resp={};\n\
             @(negedge clk);\n",
            c.aw_valid as u8,
            c.aw_ready as u8,
            c.aw.id.raw(),
            c.aw.addr.raw(),
            c.aw.len.raw(),
            c.aw.size.raw(),
            encode_burst(c.aw.burst).raw(),
            c.w_valid as u8,
            c.w_ready as u8,
            c.w.data.raw(),
            c.w.strb.raw(),
            c.w.last as u8,
            c.b_valid as u8,
            c.b_ready as u8,
            c.b.id.raw(),
            rhdl_fpga::axi4::types::encode_response(c.b.resp).raw(),
        ));
    }
    s.push_str(
        "@(posedge clk); #1;\n\
         $display(\"VERDICT %0d %0d %0d\", fault, faults, unchecked);\n\
         $finish;\n end\nendmodule\n",
    );
    s
}

/// Tier B's verdict, by compiling and running the hand-written monitor.
fn tier_b(v: Violation, dir: &std::path::Path) -> (u32, u128, u128) {
    let cycles = write_transcript::<A, DW, STRB, ID>(BUS_BYTES, v);
    let tb = dir.join(format!("tb_{v:?}.v"));
    let exe = dir.join(format!("tb_{v:?}.vvp"));
    let mut f = std::fs::File::create(&tb).unwrap();
    f.write_all(write_testbench(&cycles).as_bytes()).unwrap();
    drop(f);

    let monitor = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("verilog")
        .join("axi4")
        .join("axi4_write_monitor.v");

    let out = std::process::Command::new("iverilog")
        .arg("-g2001")
        .arg("-o")
        .arg(&exe)
        .arg(&tb)
        .arg(&monitor)
        .output()
        .expect("iverilog should be runnable; the precondition test covers absence");
    assert!(
        out.status.success(),
        "iverilog failed for {v:?}:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let run = std::process::Command::new("vvp")
        .arg(&exe)
        .output()
        .expect("vvp should be runnable");
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
        let b = tier_b(v, &dir);
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
    let uut = Axi4ReadMonitor::<A, DW, STRB, ID>::default();
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
