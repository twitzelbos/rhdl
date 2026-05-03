//! ContrAlto cycle-equivalent lockstep harness (Phase 3.5 Step 5).
//!
//! Runs both ContrAlto (via the headless `contralto-trace` tool) and
//! our `AltoChip` for the same boot scenario, then compares per-cycle
//! state.  Reports the first divergence cycle with both sides' state
//! side-by-side.
//!
//! ContrAlto is the ground truth.  Our chip's deviation tells us
//! exactly where to fix the next bug.
//!
//! Run with:
//!   cargo test -p rhdl-alto --test contralto_lockstep -- --nocapture --include-ignored
//!
//! Skipped in normal `cargo test` runs because it needs the
//! `contralto-trace` binary built (`dotnet build crates/rhdl-alto/
//! tools/contralto-trace -c Release`) and the disk image present.

use std::path::PathBuf;
use std::process::Command;

/// One cycle's CPU state, parsed from a TSV row produced by either
/// the contralto-trace tool or our chip's trace dump.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CycleState {
    task: i32,
    mpc: u16,
    t: u16,
    l: u16,
    ir: u16,
    aluc0: u8,
    r: [u16; 32],
}

fn parse_tsv_line(line: &str) -> Option<CycleState> {
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 7 + 32 { return None; }
    let parse_hex = |s: &str| -> Option<u16> {
        if let Some(stripped) = s.strip_prefix("0x") {
            u16::from_str_radix(stripped, 16).ok()
        } else { s.parse().ok() }
    };
    let task = parts[1].parse().ok()?;
    let mpc = parse_hex(parts[2])?;
    let t = parse_hex(parts[3])?;
    let l = parse_hex(parts[4])?;
    let ir = parse_hex(parts[5])?;
    let aluc0: u8 = parts[6].parse().ok()?;
    let mut r = [0u16; 32];
    for i in 0..32 {
        r[i] = parse_hex(parts[7 + i])?;
    }
    Some(CycleState { task, mpc, t, l, ir, aluc0, r })
}

fn run_contralto(disk: &str, rom_dir: &str, cycles: usize) -> Vec<CycleState> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let exe = workspace_root.join(
        "crates/rhdl-alto/tools/contralto-trace/bin/Release/net8.0/contralto-trace");
    if !exe.exists() {
        panic!("contralto-trace not built; run: dotnet build crates/rhdl-alto/tools/contralto-trace -c Release");
    }
    let output = Command::new(&exe)
        .args(["--disk", disk, "--rom-dir", rom_dir, "--cycles", &cycles.to_string()])
        .current_dir(workspace_root)
        .output()
        .expect("spawn contralto-trace");
    if !output.status.success() {
        panic!("contralto-trace failed: stderr={}",
            String::from_utf8_lossy(&output.stderr));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().skip(1)  // skip header
        .filter_map(parse_tsv_line)
        .collect()
}

fn run_rhdl_chip(disk: &str, rom_dir: &str, cycles: usize) -> Vec<CycleState> {
    use rhdl::prelude::*;
    use rhdl_alto::alto_chip::{AltoChip, ChipIn};
    use rhdl_alto::{disk_image_loader, microcode_loader};

    let microcode = microcode_loader::load_alto_ii_microcode_from_dir(
        std::path::Path::new(rom_dir)).unwrap();
    let constants = microcode_loader::load_alto_ii_constant_rom_from_dir(
        std::path::Path::new(rom_dir)).unwrap();
    let disk_image = disk_image_loader::load_disk_image_from_file(
        std::path::Path::new(disk)).unwrap();
    let boot_sector = disk_image.sector(0, 0, 0);
    // For lockstep against ContrAlto we use a SHORT 256-cycle disk
    // sector period so the chip's first sector_mark fires within a
    // few hundred cycles.  The spec-correct period (~19,608 cycles)
    // is the real-hardware cadence (`SECTOR_PERIOD_CYCLES`); using it
    // here would push the first divergence-or-match point past 20,000
    // cycles per run and make iteration slow.  ContrAlto schedules
    // the first SectorCallback at time 0 anyway (a different
    // simulation shortcut); both choices are simulation policy.
    let uut = AltoChip::with_microcode_constants_boot_and_test_disk_period(
        &microcode, &constants, &boot_sector.data, &boot_sector.label, 256,
    );
    let inputs: Vec<ChipIn> = (0..cycles).map(|_| ChipIn {
        wakeups: bits::<16>(0x0001),
    }).collect();
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let trace: Vec<_> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();

    // R-state is now exposed via ChipOut.regs — capture per-cycle so
    // the lockstep harness can compare R[0..32] against ContrAlto.
    // aluc0 is still not exposed; leave as 0 (lockstep doesn't compare
    // it currently).
    trace.into_iter().map(|t| {
        let mut r = [0u16; 32];
        for i in 0..32 {
            r[i] = t.regs[i].raw() as u16;
        }
        CycleState {
            task: t.current_task.raw() as i32,
            mpc: t.mpc.raw() as u16,
            t: t.t.raw() as u16,
            l: t.l.raw() as u16,
            ir: t.ir.raw() as u16,
            aluc0: 0,
            r,
        }
    }).collect()
}

#[test]
#[ignore]
fn lockstep_first_divergence() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let disk = manifest_dir.join("assets/disk/nonprog.dsk");
    let rom_dir = manifest_dir.join("assets/rom");
    if !disk.exists() {
        eprintln!("[lockstep] skipping — disk image absent at {disk:?}");
        return;
    }
    if !rom_dir.join("U55").exists() {
        eprintln!("[lockstep] skipping — PROM assets absent at {rom_dir:?}");
        return;
    }

    // 2000 cycles is enough for our chip's Disk Sector task to fire
    // (at cycle 256, per the 256-cycle test disk period the lockstep
    // harness configures) AND for the boot DMA to populate memory.
    // The chip uses spec-correct 19,608 cycles by default; the
    // lockstep harness specifically opts into the test period (see
    // `run_rhdl_chip` above) so this iteration loop stays fast.
    let cycles = 2000;
    let ctr_full = run_contralto(disk.to_str().unwrap(), rom_dir.to_str().unwrap(), cycles);
    let ours = run_rhdl_chip(disk.to_str().unwrap(), rom_dir.to_str().unwrap(), cycles);

    // Cycle-numbering convention differs: ContrAlto reports "MPC about
    // to execute" (post-prefetch).  Our chip reports "MPC currently
    // executing".  These differ by one cycle.  Compensate by shifting
    // ContrAlto's window left by 1 — i.e., compare ours[k] to ctr[k+1].
    // This aligns: at cycle 0, both should describe the same executing
    // microinstruction.
    eprintln!("[lockstep] ContrAlto={} cycles, ours={} cycles (1-cycle offset)",
        ctr_full.len(), ours.len());

    // ContrAlto reports "MPC about to execute" (= MPC of the
    // instruction the engine WILL run next).  Our chip reports "MPC
    // currently executing".  These differ by one cycle: ContrAlto's
    // cycle k corresponds to our cycle k-1 (= the instruction whose
    // execution produced ContrAlto's reported state).  Equivalently,
    // ContrAlto's first reported MPC (cycle 0 = 0x152) is the NEXT
    // field of NOVEM (URom[0].next = 0x152), describing the
    // instruction that will execute SECOND, not first.
    //
    // ContrAlto also models §4.4 memory-suspend stalls (which our chip
    // does NOT yet — deferred audit item D2/D3).  This shows up as
    // duplicated consecutive ContrAlto cycles where MPC stays the
    // same.  Filter those out before comparing.
    let mut ctr: Vec<&CycleState> = Vec::new();
    let mut prev_mpc: i32 = -1;
    let mut prev_task: i32 = -1;
    // Skip ctr[0] (pre-execution state) — we want ctr[1] onward as
    // "MPC of the k-th instruction executed".
    for c in ctr_full.iter().skip(1) {
        if c.mpc as i32 == prev_mpc && c.task == prev_task {
            continue;  // memory-stall duplicate
        }
        ctr.push(c);
        prev_mpc = c.mpc as i32;
        prev_task = c.task;
    }
    // Now ctr[k].mpc IS the MPC that ContrAlto's engine has just
    // finished executing at "cycle k+1" — equivalent to "MPC executed
    // at our chip's cycle k+1".  But we want ours[k] to compare to
    // ctr[k] (both = "MPC executed at cycle k").  Skip ours[0] (NOVEM
    // at MPC=0) — its NEXT field IS what ContrAlto reported as its
    // first ctr[0] entry (post-skip).
    let our_skip = 1;

    // Two independent indices.  When (task, mpc) match, both advance.
    // When they diverge, we scan forward in BOTH traces for the next
    // re-sync point: a (task, mpc) pair that appears in BOTH traces
    // within the search window.  This lets us skip past simulation-
    // policy differences (sector_mark timing, memory stalls, task-
    // arbitration phase) and find the NEXT real divergence.
    //
    // Re-sync pattern: when we detect a divergence at (i_ctr, i_ours),
    // scan forward in `ours` for the first cycle (i_ours_next) where
    // (task, mpc) matches ctr[i_ctr] (or any of the next K ctr entries).
    // Report what was skipped.

    const SKIP_WINDOW: usize = 500;
    let mut i_ctr = 0usize;
    let mut i_ours = our_skip;
    let mut matched = 0usize;
    let mut divergences = 0usize;
    let max_divergences = 5;

    let mut r_divergences: Vec<(usize, usize, usize, u16, u16)> = Vec::new();
    let max_r_divergences_to_report = 5;
    while i_ctr < ctr.len() && i_ours < ours.len() {
        let c = ctr[i_ctr];
        let o = &ours[i_ours];
        if c.task == o.task && c.mpc == o.mpc {
            // (task, mpc) match — also check R-state.  Per the F2-NEXT-
            // modifier-timing-fix CHANGELOG entry, the next-most-likely
            // bug is in disk-task R-register accumulation, which would
            // show up here as the FIRST cycle where (task, mpc) match
            // but R[i] differs.
            for ri in 0..32 {
                if c.r[ri] != o.r[ri] && r_divergences.len() < max_r_divergences_to_report {
                    r_divergences.push((i_ctr, i_ours, ri, c.r[ri], o.r[ri]));
                }
            }
            matched += 1;
            i_ctr += 1;
            i_ours += 1;
            continue;
        }
        // Divergence — try to re-sync.
        divergences += 1;
        eprintln!("\n[lockstep] DIVERGENCE #{divergences} (matched {matched} so far):");
        eprintln!("  CTR[{i_ctr}]:  task={} mpc=0x{:03x} T=0x{:04x} L=0x{:04x} IR=0x{:04x}",
            c.task, c.mpc, c.t, c.l, c.ir);
        eprintln!("  OURS[{i_ours}]: task={} mpc=0x{:03x} T=0x{:04x} L=0x{:04x} IR=0x{:04x}",
            o.task, o.mpc, o.t, o.l, o.ir);

        // Try to re-sync.  Search forward in `ours` for the first
        // cycle that matches ctr[i_ctr]'s (task, mpc).  If not found,
        // try the next ctr entry, etc.
        let mut resync_found = false;
        let max_ctr_advance = SKIP_WINDOW.min(ctr.len() - i_ctr);
        'outer: for ctr_advance in 0..max_ctr_advance {
            let target = ctr[i_ctr + ctr_advance];
            let max_ours_advance = SKIP_WINDOW.min(ours.len() - i_ours);
            for ours_advance in 0..max_ours_advance {
                let o2 = &ours[i_ours + ours_advance];
                if o2.task == target.task && o2.mpc == target.mpc {
                    eprintln!("[lockstep] resync: CTR advanced {ctr_advance}, OURS advanced {ours_advance} → matched at task={} mpc=0x{:03x}",
                        target.task, target.mpc);
                    i_ctr += ctr_advance;
                    i_ours += ours_advance;
                    resync_found = true;
                    break 'outer;
                }
            }
        }
        if !resync_found {
            eprintln!("[lockstep] no resync found in {SKIP_WINDOW}-cycle window — stopping comparison");
            break;
        }
        if divergences >= max_divergences {
            eprintln!("[lockstep] {max_divergences} divergences reported — stopping");
            break;
        }
    }
    eprintln!("\n[lockstep] summary: {matched} matched (task, mpc) pairs, {divergences} divergence events");
    if !r_divergences.is_empty() {
        eprintln!("\n[lockstep] R-state divergences (where (task, mpc) match but R[i] differs):");
        for (i_ctr, i_ours, ri, ctr_v, ours_v) in &r_divergences {
            eprintln!(
                "  CTR[{i_ctr}]/OURS[{i_ours}] R[{ri}]: CTR=0x{ctr_v:04x}  OURS=0x{ours_v:04x}",
            );
        }
        if r_divergences.len() == max_r_divergences_to_report {
            eprintln!("  (capped at {max_r_divergences_to_report}; more may exist)");
        }
    } else {
        eprintln!("\n[lockstep] no R-state divergences observed at any matched (task, mpc) pair.");
    }
}
