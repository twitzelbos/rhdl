//! Dump microinstructions at the MPCs surrounding the lockstep
//! divergences, so we can identify which F1/F2 NEXT-modifier our
//! implementation handles differently from ContrAlto.
//!
//! Divergence #2: CTR mpc=0x154, OURS mpc=0x155 (T=L=IR=0x0001) — both
//! came from a predecessor whose NEXT field equals 0x154; CTR keeps it,
//! ours OR's bit 0.
//! Divergence #3: CTR mpc=0x17e, OURS mpc=0x17f.

use rhdl_alto::isa::Microinstruction;
use rhdl_alto::microcode_loader;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from("crates/rhdl-alto/assets/rom");
    if !dir.join("U55").exists() {
        eprintln!("[skip] PROM assets missing under {dir:?}");
        return;
    }
    let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&dir).unwrap();

    let interesting = [
        // Divergence #2 neighborhood
        0x150, 0x151, 0x152, 0x153, 0x154, 0x155, 0x156, // Divergence #3 neighborhood
        0x17a, 0x17b, 0x17c, 0x17d, 0x17e, 0x17f, 0x180,
    ];

    // Also: print ALL microinstructions whose NEXT lands at the
    // divergence MPC pair (0x154/0x155 or 0x17e/0x17f) — these are
    // the predecessors that produce our off-by-one.
    let divergent_targets: [u16; 4] = [0x154, 0x155, 0x17e, 0x17f];

    println!("=== Microinstructions at neighborhood MPCs ===");
    for &mpc in &interesting {
        let raw = microcode[mpc];
        let mi = Microinstruction::unpack(raw);
        println!(
            "MPC=0x{mpc:03x} raw=0x{raw:08x}  rsel={:>2} aluf={:?} bs={:?} f1={:?} f2={:?} t={} l={} next=0x{:03x}",
            mi.rsel.raw(),
            mi.aluf,
            mi.bs,
            mi.f1,
            mi.f2,
            mi.t_load as u8,
            mi.l_load as u8,
            mi.next.raw(),
        );
    }

    println!("\n=== Predecessors that point to a divergent MPC pair ===");
    for (mpc_idx, &raw) in microcode.iter().enumerate() {
        let mi = Microinstruction::unpack(raw);
        let n = mi.next.raw() as u16;
        let n_pair_lo = n & !1; // either side of the pair
        if divergent_targets.iter().any(|&t| (t & !1) == n_pair_lo) {
            println!(
                "MPC=0x{mpc_idx:03x} → next=0x{n:03x}  rsel={:>2} aluf={:?} bs={:?} f1={:?} f2={:?} t={} l={}",
                mi.rsel.raw(),
                mi.aluf,
                mi.bs,
                mi.f1,
                mi.f2,
                mi.t_load as u8,
                mi.l_load as u8,
            );
        }
    }
}
