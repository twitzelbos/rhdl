//! Canonical R-register name aliases per `altoIIcode3.mu` /
//! `altoconsts23.mu`, indexed for diagnostic readability.
//!
//! **Not used by the chip kernel.**  Aliases are resolved upstream by
//! the Mu-assembler when building `.mb` binaries; the chip operates on
//! bare RSEL fields.  This module exists purely to make diagnostic
//! dumpers and lockstep traces human-readable: `$PC=0x0008` is
//! immediately interpretable; `R[6]=0x0008` requires knowing the
//! alias table.
//!
//! See `alto-processor-and-microcode-spec.md` §4.2 + §4.2.1 for the
//! full table.  This module is the **source-of-truth machine-readable
//! mirror** of those tables — keep them in sync.
//!
//! Some R-slots are aliased differently per microcode region (e.g.,
//! R[3] is `$AC0` in the Emulator and `$SKEW` in BitBLT).  Callers
//! provide the running `task` so the resolver can pick the appropriate
//! alias.  When multiple aliases share a slot in the same task, the
//! most common is returned and others noted in the spec digest.

/// Return the canonical alias for register `R[index]` when the running
/// `task` is `task` (0=Emulator, 4=Disk Sector, 8=MRT, 9=Display Word,
/// 10=Cursor, 11=Display Horizontal, 12=Display Vertical, 14=Disk
/// Word).  Returns the bare `R[N]` form if no alias is canonical.
///
/// Decimal index in (0..32).  Spec digest §4.2.1 has the full
/// alphabetical table.
pub fn r_alias(task: u8, index: usize) -> &'static str {
    if index >= 32 {
        return "R[??]";
    }
    // Universal aliases (same in every task).
    if let Some(name) = universal_alias(index) {
        return name;
    }
    // Task-specific aliases.
    match task {
        0 => emulator_alias(index),
        4 | 14 => disk_alias(index),
        // Display tasks share the display R-slots (R[16..24] octal 20..30).
        9 | 10 | 11 | 12 => display_alias(index),
        _ => fallback(index),
    }
}

fn universal_alias(index: usize) -> Option<&'static str> {
    match index {
        4  => Some("$NWW"),     // interrupt-system state
        21 => Some("$MTEMP"),   // public temp (R[25 octal])
        31 => Some("$R37"),     // MRT/timer/EIA
        _  => None,
    }
}

fn emulator_alias(index: usize) -> &'static str {
    // Per altoIIcode3.mu — the Emulator's R-aliases.
    match index {
        0 => "$AC3",
        1 => "$AC2",
        2 => "$AC1",
        3 => "$AC0",
        5 => "$SAD",      // also $CYRET, $TEMP — SAD is most common in Emulator boot
        6 => "$PC",
        7 => "$XREG",     // also $CYCOUT, $WIDTH, $PLIER
        8 => "$XH",       // BLT loop counter
        9 => "$CLOCKTEMP",
        _ => fallback(index),
    }
}

fn disk_alias(index: usize) -> &'static str {
    // Per altoIIcode3.mu — Disk Sector / Disk Word aliases.
    match index {
        25 => "$KWDCT",
        26 => "$CKSUMR",
        27 => "$KNMAR",
        28 => "$DCBR",
        _  => fallback(index),
    }
}

fn display_alias(index: usize) -> &'static str {
    match index {
        16 => "$CURX",
        17 => "$CURDATA",
        18 => "$CBA",
        19 => "$AECL",
        20 => "$SLC",
        22 => "$HTAB",
        23 => "$YPOS",
        24 => "$DWA",
        29 => "$DWAX",
        _  => fallback(index),
    }
}

fn fallback(index: usize) -> &'static str {
    // Static strings for indices without a canonical alias.
    static FALLBACKS: [&str; 32] = [
        "R[0]",  "R[1]",  "R[2]",  "R[3]",  "R[4]",  "R[5]",  "R[6]",  "R[7]",
        "R[8]",  "R[9]",  "R[10]", "R[11]", "R[12]", "R[13]", "R[14]", "R[15]",
        "R[16]", "R[17]", "R[18]", "R[19]", "R[20]", "R[21]", "R[22]", "R[23]",
        "R[24]", "R[25]", "R[26]", "R[27]", "R[28]", "R[29]", "R[30]", "R[31]",
    ];
    FALLBACKS[index]
}

/// Format an R-register read in `<alias>=0x<value>` form, or
/// `R[<i>]=0x<value>` if no canonical alias for the task.
///
/// Convenience for diagnostic dumpers.
pub fn format_r(task: u8, index: usize, value: u16) -> String {
    let alias = r_alias(task, index);
    format!("{alias}=0x{value:04x}")
}

/// Like [`r_alias`] but if the current task has no specific alias for
/// this slot, fall back to the Emulator alias when one exists.  Many
/// R-slots get written by the Emulator (PC, SAD, XH, AC0..3) and then
/// read by other tasks — the Emulator alias remains the most
/// informative name for those cross-task observations.
///
/// Use in lockstep / cross-task diagnostics.  Use [`r_alias`] when you
/// want strictly the current task's alias.
pub fn r_alias_with_emulator_fallback(task: u8, index: usize) -> &'static str {
    if index >= 32 {
        return "R[??]";
    }
    let primary = r_alias(task, index);
    // If the primary alias is just the bare `R[N]` form AND the
    // Emulator has a real symbolic alias for this slot, use that.
    if !primary.starts_with('$') && task != 0 {
        let emu = emulator_alias(index);
        if emu.starts_with('$') {
            return emu;
        }
    }
    primary
}

/// Format-like [`format_r`] but with cross-task fallback.  If the
/// current task has no symbolic alias for the slot, the Emulator's
/// alias (when one exists) is shown with an `@Emul` annotation.
pub fn format_r_with_context(task: u8, index: usize, value: u16) -> String {
    if index >= 32 {
        return format!("R[??]=0x{value:04x}");
    }
    let primary = r_alias(task, index);
    if primary.starts_with('$') {
        // Current task has a real alias.
        return format!("{primary}=0x{value:04x}");
    }
    if task != 0 {
        let emu = emulator_alias(index);
        if emu.starts_with('$') {
            return format!("{emu}@Emul=0x{value:04x}");
        }
    }
    format!("R[{index}]=0x{value:04x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emulator_critical_aliases() {
        assert_eq!(r_alias(0, 0), "$AC3");
        assert_eq!(r_alias(0, 1), "$AC2");
        assert_eq!(r_alias(0, 2), "$AC1");
        assert_eq!(r_alias(0, 3), "$AC0");
        assert_eq!(r_alias(0, 4), "$NWW");
        assert_eq!(r_alias(0, 5), "$SAD");
        assert_eq!(r_alias(0, 6), "$PC");
        assert_eq!(r_alias(0, 7), "$XREG");
        assert_eq!(r_alias(0, 8), "$XH");
    }

    #[test]
    fn disk_task_aliases() {
        assert_eq!(r_alias(4, 25), "$KWDCT");
        assert_eq!(r_alias(4, 26), "$CKSUMR");
        assert_eq!(r_alias(4, 27), "$KNMAR");
        assert_eq!(r_alias(4, 28), "$DCBR");
        assert_eq!(r_alias(14, 28), "$DCBR"); // Disk Word task
    }

    #[test]
    fn display_aliases() {
        assert_eq!(r_alias(12, 16), "$CURX");
        assert_eq!(r_alias(12, 18), "$CBA");
        assert_eq!(r_alias(11, 22), "$HTAB");
        assert_eq!(r_alias(9, 24), "$DWA");
    }

    #[test]
    fn universal_aliases_are_task_independent() {
        for task in [0u8, 4, 8, 14] {
            assert_eq!(r_alias(task, 4), "$NWW");
            assert_eq!(r_alias(task, 21), "$MTEMP");
            assert_eq!(r_alias(task, 31), "$R37");
        }
    }

    #[test]
    fn fallback_for_unaliased_slots() {
        assert_eq!(r_alias(0, 13), "R[13]");
        assert_eq!(r_alias(4, 0), "R[0]");  // AC3 in Emulator, no alias in Disk
    }

    #[test]
    fn format_r_renders_alias_with_value() {
        assert_eq!(format_r(0, 6, 0x0008), "$PC=0x0008");
        assert_eq!(format_r(0, 8, 0xffff), "$XH=0xffff");
        assert_eq!(format_r(4, 26, 0xc0de), "$CKSUMR=0xc0de");
        assert_eq!(format_r(0, 13, 0x1234), "R[13]=0x1234");
    }

    #[test]
    fn out_of_range_returns_safe_marker() {
        assert_eq!(r_alias(0, 99), "R[??]");
    }

    #[test]
    fn cross_task_fallback_uses_emulator_alias() {
        // R[6] in Disk Sector context: no disk alias, falls back to $PC.
        assert_eq!(r_alias_with_emulator_fallback(4, 6), "$PC");
        assert_eq!(r_alias_with_emulator_fallback(4, 8), "$XH");
        assert_eq!(r_alias_with_emulator_fallback(4, 5), "$SAD");
        // R[28] in Disk task has $DCBR — current-task wins, no fallback.
        assert_eq!(r_alias_with_emulator_fallback(4, 28), "$DCBR");
        // R[13] has no alias anywhere — bare R[13].
        assert_eq!(r_alias_with_emulator_fallback(4, 13), "R[13]");
    }

    #[test]
    fn format_r_with_context_annotates_cross_task() {
        // Disk task reading R[6]: shows $PC@Emul to flag cross-task.
        assert_eq!(format_r_with_context(4, 6, 0x0008), "$PC@Emul=0x0008");
        // Emulator reading R[6]: shows $PC (no annotation).
        assert_eq!(format_r_with_context(0, 6, 0x0008), "$PC=0x0008");
        // Disk task reading R[28] (DCBR): native alias, no annotation.
        assert_eq!(format_r_with_context(4, 28, 0x1234), "$DCBR=0x1234");
        // Bare slot, no alias anywhere.
        assert_eq!(format_r_with_context(4, 13, 0x5678), "R[13]=0x5678");
    }
}
