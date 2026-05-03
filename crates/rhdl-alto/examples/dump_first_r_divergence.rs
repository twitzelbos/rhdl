//! Per-cycle R-state side-by-side dumper that finds the FIRST cycle
//! where any R-register diverges between OUR chip and ContrAlto.
//!
//! Higher-precision diagnostic than `tests/contralto_lockstep.rs`,
//! which only samples R[] at matched (task, mpc) pairs (and so misses
//! the cycle where the divergence is INTRODUCED).
//!
//! Both sides are run with sector_mark firing on cycle 1 (matching
//! ContrAlto's `_sectorEvent = new Event(0, ...)` choice) so the only
//! variables left are microengine + task-arbitration semantics.
//!
//! For each cycle 0..N:
//!   1. Read CTR's R[0..32] from contralto-trace TSV.
//!   2. Read OUR chip's R[0..32] from ChipOut.regs.
//!   3. Diff slot-by-slot.  Report the FIRST cycle with any
//!      mismatch, with surrounding context (MPCs, tasks, IRs).
//!
//! This finds the IMMEDIATE divergence cause — typically one
//! microinstruction whose effect on R[i] differs between sims.
//!
//! Run with (after `dotnet build crates/rhdl-alto/tools/contralto-trace -c Release`):
//!   cargo run --example dump_first_r_divergence --package rhdl-alto

use rhdl::prelude::*;
use rhdl_alto::alto_chip::{AltoChip, ChipIn};
use rhdl_alto::{disk_image_loader, microcode_loader};
use rhdl_alto::register_aliases::r_alias_with_emulator_fallback;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone)]
struct Snap {
    cycle: usize,
    task: u8,
    mpc: u16,
    t: u16,
    l: u16,
    ir: u16,
    r: [u16; 32],
    /// True if this cycle was an OURS-side memory-pipeline stall (the
    /// engine froze and didn't advance MPC).  Always false for CTR
    /// (ContrAlto's TSV doesn't expose stall state).
    mem_stall: bool,
}

fn parse_tsv(stdout: &str) -> Vec<Snap> {
    stdout.lines().skip(1)
        .enumerate()
        .filter_map(|(_idx, line)| {
            let p: Vec<&str> = line.split('\t').collect();
            if p.len() < 7 + 32 { return None; }
            let parse_hex = |s: &str| -> Option<u16> {
                if let Some(rest) = s.strip_prefix("0x") {
                    u16::from_str_radix(rest, 16).ok()
                } else {
                    s.parse().ok()
                }
            };
            let cycle: usize = p[0].parse().ok()?;
            let task: u8 = p[1].parse().ok()?;
            let mpc = parse_hex(p[2])?;
            let t = parse_hex(p[3])?;
            let l = parse_hex(p[4])?;
            let ir = parse_hex(p[5])?;
            let mut r = [0u16; 32];
            for i in 0..32 {
                r[i] = parse_hex(p[7 + i])?;
            }
            Some(Snap { cycle, task, mpc, t, l, ir, r, mem_stall: false })
        })
        .collect()
}

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let disk = manifest_dir.join("assets/disk/nonprog.dsk");
    let rom_dir = manifest_dir.join("assets/rom");
    if !disk.exists() {
        eprintln!("[skip] no disk image at {disk:?}");
        return;
    }
    if !rom_dir.join("U55").exists() {
        eprintln!("[skip] no PROM assets at {rom_dir:?}");
        return;
    }

    const CYCLES: usize = 300;

    // ---- our chip ----
    let microcode =
        microcode_loader::load_alto_ii_microcode_from_dir(&rom_dir).unwrap();
    let constants =
        microcode_loader::load_alto_ii_constant_rom_from_dir(&rom_dir).unwrap();
    let disk_image =
        disk_image_loader::load_disk_image_from_file(&disk).unwrap();
    let boot_sector = disk_image.sector(0, 0, 0);

    let uut = AltoChip::with_microcode_constants_boot_and_test_disk_period_at_boundary(
        &microcode, &constants, &boot_sector.data, &boot_sector.label, 256,
    );
    let inputs: Vec<ChipIn> = (0..CYCLES)
        .map(|_| ChipIn { wakeups: bits::<16>(0x0001) })
        .collect();
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let ours_raw: Vec<_> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    let ours: Vec<Snap> = ours_raw.iter().enumerate().map(|(i, t)| {
        let mut r = [0u16; 32];
        for k in 0..32 { r[k] = t.regs[k].raw() as u16; }
        Snap {
            cycle: i,
            task: t.current_task.raw() as u8,
            mpc: t.mpc.raw() as u16,
            t: t.t.raw() as u16,
            l: t.l.raw() as u16,
            ir: t.ir.raw() as u16,
            r,
            mem_stall: t.mem_stall,
        }
    }).collect();

    // (task-transition timelines printed below, after ctr is parsed)

    // ---- ContrAlto ----
    let exe = workspace_root.join(
        "crates/rhdl-alto/tools/contralto-trace/bin/Release/net8.0/contralto-trace",
    );
    if !exe.exists() {
        eprintln!("[skip] contralto-trace not built — run `dotnet build crates/rhdl-alto/tools/contralto-trace -c Release`");
        return;
    }
    let output = Command::new(&exe)
        .args([
            "--disk", disk.to_str().unwrap(),
            "--rom-dir", rom_dir.to_str().unwrap(),
            "--cycles", &CYCLES.to_string(),
        ])
        .current_dir(workspace_root)
        .output()
        .expect("spawn contralto-trace");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let ctr = parse_tsv(&stdout);

    // ---- task-transition timelines, both sides ----
    println!("=== OURS' task transitions ===");
    let mut prev: i32 = -1;
    for (i, t) in ours_raw.iter().enumerate() {
        let task = t.current_task.raw() as i32;
        if task != prev {
            println!("  cycle {i:3} → task {task}{}",
                if prev < 0 { " (init)".to_string() } else { format!(" (was {prev})") });
            prev = task;
        }
    }
    println!();
    println!("=== CTR's task transitions ===");
    let mut prev: i32 = -1;
    for c in &ctr {
        let task = c.task as i32;
        if task != prev {
            println!("  cycle {:3} → task {task}{}",
                c.cycle,
                if prev < 0 { " (init)".to_string() } else { format!(" (was {prev})") });
            prev = task;
        }
    }
    println!();

    // ---- find FIRST per-cycle MPC divergence (within same-task windows) ----
    // R-divergences are downstream symptoms.  The PRIMARY cause is
    // when the two sims execute different microinstructions in the
    // same task at the same cycle — i.e., MPC diverges.  Find that
    // FIRST.
    println!("=== FIRST per-cycle MPC divergence (within matched task) ===");
    let n_aligned = ctr.len().min(ours.len());
    let mut first_mpc_div: Option<usize> = None;
    for k in 0..n_aligned {
        let c = &ctr[k];
        let o = &ours[k];
        if c.task == o.task && c.mpc != o.mpc {
            first_mpc_div = Some(k);
            break;
        }
    }
    match first_mpc_div {
        Some(k) => {
            println!("First MPC divergence at cycle {k}:");
            let lo = k.saturating_sub(2);
            let hi = (k + 4).min(n_aligned - 1);
            for cyc in lo..=hi {
                let c = &ctr[cyc];
                let o = &ours[cyc];
                let mark = if cyc == k { " ←" } else { "" };
                let same_task = c.task == o.task;
                let same_mpc = c.mpc == o.mpc;
                let status = match (same_task, same_mpc) {
                    (true, true)   => "OK",
                    (true, false)  => "MPC DIFFERS",
                    (false, _)     => "DIFFERENT TASK",
                };
                println!(
                    "  cycle {cyc}: CTR task={} mpc=0x{:03x}  OURS task={} mpc=0x{:03x}  [{status}]{mark}",
                    c.task, c.mpc, o.task, o.mpc,
                );
            }
        }
        None => {
            println!("No within-task MPC divergence in first {n_aligned} cycles.");
        }
    }
    println!();

    // ---- full per-cycle MPC trace ----
    // The "first MPC divergence" section above stops at cycle 0 due to
    // OURS' startup MPC-reporting artifact (task_started gating + URom
    // 1-cycle latency interaction).  Cycles 0-3 of OURS report mpc=0
    // even though the engine is internally at NOVEM → 0x152 → 0x153 →
    // 0x154.  By cycle 4 (KSEC start) the reporting is consistent.
    //
    // Print the FULL cycle-by-cycle MPC trace so we can see what
    // happens AFTER the startup transient — specifically, where in
    // KSEC's run window OURS and CTR take different paths.
    println!("=== Full per-cycle MPC trace (cycles 0..{}) ===", n_aligned);
    println!("Notation: '*' marks cycles where MPC OR task differs.");
    println!("Cycles 0-3: OURS reports mpc=0 due to task_started + URom-latency display");
    println!("artifact.  Real divergence (if any) emerges at cycle 4 (KSEC start) onward.");
    println!();
    println!("{:>5}  {:>10}  {:>10}  {}", "cyc", "CTR", "OURS", "");
    let mut last_ctr_task: i32 = -1;
    let mut last_ours_task: i32 = -1;
    for k in 0..n_aligned {
        let c = &ctr[k];
        let o = &ours[k];
        let same = c.task == o.task && c.mpc == o.mpc;
        let mark = if same { "" } else { " *" };
        let ctr_yield = if (c.task as i32) != last_ctr_task && last_ctr_task >= 0 {
            format!("CTR yield {}→{}", last_ctr_task, c.task)
        } else { String::new() };
        let ours_yield = if (o.task as i32) != last_ours_task && last_ours_task >= 0 {
            format!("OURS yield {}→{}", last_ours_task, o.task)
        } else { String::new() };
        let yields = match (ctr_yield.is_empty(), ours_yield.is_empty()) {
            (true, true) => String::new(),
            (false, true) => format!("    [{ctr_yield}]"),
            (true, false) => format!("    [{ours_yield}]"),
            (false, false) => format!("    [{ctr_yield}; {ours_yield}]"),
        };
        let stall_marker = if o.mem_stall { " [STALL]" } else { "" };
        println!(
            "{k:>5}  t{}/0x{:03x}  t{}/0x{:03x}{}{}{}",
            c.task, c.mpc, o.task, o.mpc, mark, stall_marker, yields,
        );
        last_ctr_task = c.task as i32;
        last_ours_task = o.task as i32;
    }
    println!();

    // ---- find first WITHIN-TASK MPC divergence after cycle 4 ----
    // (Skip the cycles 0-3 startup-transient artifact.)
    println!("=== FIRST within-task MPC divergence at cycle ≥ 4 ===");
    let mut first_real: Option<usize> = None;
    for k in 4..n_aligned {
        let c = &ctr[k];
        let o = &ours[k];
        if c.task == o.task && c.mpc != o.mpc {
            first_real = Some(k);
            break;
        }
    }
    match first_real {
        Some(k) => {
            println!("First real MPC divergence at cycle {k}:");
            let lo = k.saturating_sub(2);
            let hi = (k + 6).min(n_aligned - 1);
            for cyc in lo..=hi {
                let c = &ctr[cyc];
                let o = &ours[cyc];
                let same_task = c.task == o.task;
                let same_mpc = c.mpc == o.mpc;
                let status = match (same_task, same_mpc) {
                    (true, true)   => "OK",
                    (true, false)  => "MPC DIFFERS — different microcode path",
                    (false, _)     => "DIFFERENT TASK",
                };
                let mark = if cyc == k { " ←" } else { "" };
                println!(
                    "  cycle {cyc}: CTR task={} mpc=0x{:03x}  OURS task={} mpc=0x{:03x}  [{status}]{mark}",
                    c.task, c.mpc, o.task, o.mpc,
                );
            }
            println!();
            println!("Interpretation: at cycle {}-1 (= last matched cycle), both sims",
                k);
            println!("are at the same (task, mpc) — i.e., about to execute the same");
            println!("microinstruction.  At cycle {}, they're at DIFFERENT MPCs.  This means", k);
            println!("the microinstruction at cycle {}-1 had a CONDITIONAL NEXT (F2 dispatch,", k);
            println!("F2 NEXT-modifier, or branch-on-condition) that selected differently in");
            println!("the two sims.  Suspect: that microinstruction's F1/F2 dispatch handler.");
        }
        None => {
            println!("No within-task MPC divergence in cycles 4..{}.", n_aligned);
            println!("If R-state still diverges, the cause is in WHAT each microinstruction");
            println!("does (ALU function, BS source, T/L load) rather than which one runs.");
        }
    }
    println!();

    // ---- find first R-divergence ----
    println!("=== Per-cycle R-state side-by-side ({}-cycle window) ===", CYCLES);
    println!("CTR cycles:  {}", ctr.len());
    println!("OURS cycles: {}", ours.len());
    println!();
    println!("Cycle alignment: BOTH sims report state POST-SingleStep / POST-cycle.");
    println!("  CTR[k]: TSV emits state after running cycle-k's instruction (mpc = next-to-execute).");
    println!("  OURS[k]: ChipOut.mpc = current_mpc presented for cycle k+1 (= next-to-execute).");
    println!("Both 'mpc' fields mean 'MPC about to execute' → ctr[k] vs ours[k] aligned directly.");
    println!();

    let n = ctr.len().min(ours.len());
    let mut first_div: Option<(usize, Vec<usize>)> = None;
    for k in 0..n {
        let mut diffs: Vec<usize> = Vec::new();
        for ri in 0..32 {
            if ctr[k].r[ri] != ours[k].r[ri] {
                diffs.push(ri);
            }
        }
        if !diffs.is_empty() {
            first_div = Some((k, diffs));
            break;
        }
    }

    let (k, diffs) = match first_div {
        Some(x) => x,
        None => {
            println!("✓ No R-state divergences in the first {n} cycles.  Lockstep clean!");
            return;
        }
    };

    println!("FIRST R-DIVERGENCE: cycle {k}");
    println!();
    println!("Surrounding context (cycles {}..={}):", k.saturating_sub(3), (k + 3).min(n - 1));
    println!("{:>5}  {:>4}  {:>5}  {:>6}  {:>6}  {:>6}  {:>6}  {:>6}",
        "cycle", "side", "task", "mpc", "T", "L", "IR", "");
    for cyc in k.saturating_sub(3)..=(k + 3).min(n - 1) {
        let c = &ctr[cyc];
        let o = &ours[cyc];
        let mark = if cyc == k { " ←" } else { "" };
        println!(
            "{cyc:>5}  {:>4}  {:>5}  0x{:03x}  0x{:04x}  0x{:04x}  0x{:04x}{}",
            "CTR", c.task, c.mpc, c.t, c.l, c.ir, mark,
        );
        println!(
            "{cyc:>5}  {:>4}  {:>5}  0x{:03x}  0x{:04x}  0x{:04x}  0x{:04x}{}",
            "OURS", o.task, o.mpc, o.t, o.l, o.ir, mark,
        );
    }
    println!();
    println!("R-slots that differ at cycle {k}:");
    let task = ours[k].task;
    for ri in &diffs {
        let alias = r_alias_with_emulator_fallback(task, *ri);
        let ctr_v = ctr[k].r[*ri];
        let ours_v = ours[k].r[*ri];
        let prev_ctr_v = if k > 0 { ctr[k - 1].r[*ri] } else { 0 };
        let prev_ours_v = if k > 0 { ours[k - 1].r[*ri] } else { 0 };
        let ctr_just_changed = ctr_v != prev_ctr_v;
        let ours_just_changed = ours_v != prev_ours_v;
        let change_marker = match (ctr_just_changed, ours_just_changed) {
            (true,  true ) => "BOTH changed this cycle (computed differently)",
            (true,  false) => "CTR  changed this cycle, OURS held",
            (false, true ) => "OURS changed this cycle, CTR  held",
            (false, false) => "NEITHER changed this cycle (divergence inherited from earlier)",
        };
        println!(
            "  {alias} (R[{ri}], task={task}): CTR=0x{ctr_v:04x}  OURS=0x{ours_v:04x}    [{change_marker}]",
        );
    }
    println!();
    println!("If 'OURS changed this cycle': our chip wrote a different value to R[i]");
    println!("than CTR did at this cycle.  The microinstruction at OURS[{k}].mpc is the");
    println!("immediate suspect.");
    println!("If 'CTR changed this cycle': CTR's microcode wrote and ours didn't (or wrote");
    println!("a different value).  CTR side's microinstruction is the suspect.");
    println!("If 'NEITHER changed': the divergence was introduced earlier; this cycle");
    println!("just happens to be the first MATCHING cycle index where the difference is");
    println!("visible in our sampling.");
}
