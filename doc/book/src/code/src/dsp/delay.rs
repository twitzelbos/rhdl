//! Group delay as a requirement, for a chain inside a control loop.

use rhdl_fpga::dsp::cic::chain::{ChainSpec, Unmet, design};
use rhdl_fpga::dsp::cic::compensator::Method;

// ANCHOR: spec
/// A lock-in measurement filter for a control loop: a test tone at
/// 125 Msps in, 100 ksps out, and — unlike a receiver that is only
/// listening — a hard limit on how long the answer may take to arrive.
pub fn lock_in() -> ChainSpec {
    ChainSpec {
        fs_hz: 125e6,
        decimate: 1250,
        alias_free_bw_hz: 10e3,
        input_width: 16,
        output_width: 24,
        max_ripple_db: 0.1,
        min_alias_rejection_db: 60.0,
        min_snr_db: 80.0,
        coeff_width: 16,
        max_stages: 8,
        max_taps: 31,
        max_chain_stages: 2,
        stopband_edge: 1.0,
        min_stopband_db: 0.0,
        method: Method::LeastSquares,
        // 30 us. At the `1/(10*delay)` rule of thumb that is about a
        // 3 kHz loop, which is the requirement the plant imposed.
        max_group_delay_s: 30e-6,
        pipelined_combs: true,
    }
}
// ANCHOR_END: spec

// ANCHOR: report
/// Design under the bound, and say what happened either way.
///
/// A refusal is the useful answer here: it comes with the shortest delay
/// any candidate achieved, so the gap between that and the requirement is
/// the size of the problem.
pub fn report() -> String {
    match design(lock_in()) {
        Ok(d) => format!("{d}"),
        Err(Unmet::GroupDelay { best_s, needed_s }) => format!(
            "no split is fast enough: best {:.1} us against {:.1} us asked",
            best_s * 1e6,
            needed_s * 1e6
        ),
        Err(other) => format!("{other:?}"),
    }
}
// ANCHOR_END: report

// ANCHOR: budget
/// Where the delay goes, before choosing what to attack.
///
/// Design once with no bound and read the parts: which term is largest
/// depends on the configuration, so the useful move is to look rather
/// than to assume.
pub fn budget() -> String {
    let unbounded = ChainSpec {
        max_group_delay_s: 0.0,
        ..lock_in()
    };
    match design(unbounded) {
        Ok(d) => {
            let b = d.group_delay;
            let (name, size) = b.dominant();
            format!(
                "{:.0} samples total; largest is the {name} at {size:.0}",
                b.total()
            )
        }
        Err(other) => format!("{other:?}"),
    }
}
// ANCHOR_END: budget

#[cfg(test)]
mod tests {
    use super::*;

    /// **The chapter quotes this refusal verbatim.**
    #[test]
    fn the_lock_in_spec_is_refused_on_delay() {
        assert_eq!(
            report(),
            "no split is fast enough: best 60.0 us against 30.0 us asked"
        );
    }

    /// **And this budget line.**
    #[test]
    fn the_budget_line_is_what_the_chapter_says() {
        assert_eq!(
            budget(),
            "7516 samples total; largest is the comb pipeline at 3750"
        );
    }

    /// **The per-stage figures the chapter quotes, and its central
    /// claim: the head stage is nearly free and the tail is not.**
    #[test]
    fn the_per_stage_figures_are_what_the_chapter_says() {
        use rhdl_fpga::dsp::cic::delay::decimation_stage_breakdowns;
        let d = design(ChainSpec {
            max_group_delay_s: 0.0,
            ..lock_in()
        })
        .expect("designable without the bound");
        let shapes: Vec<(usize, usize, usize)> = d
            .cics
            .iter()
            .map(|c| (c.stages, c.decimate, c.delay))
            .collect();
        assert_eq!(
            shapes,
            vec![(1, 10, 1), (4, 125, 1)],
            "the chapter names these"
        );
        let parts = decimation_stage_breakdowns(&shapes, true);
        // Half samples: the centre of mass of an even-length boxcar
        // falls between two samples. The chapter prints them rounded,
        // which is what the `{:.0}` in `budget` does too.
        assert_eq!(parts[0].total(), 5.5, "{parts:?}");
        assert_eq!(parts[0].comb_pipeline, 0.0);
        assert_eq!(parts[1].total(), 6261.0, "{parts:?}");
        assert_eq!(parts[1].comb_pipeline, 3750.0);
    }

    /// **The software figure the chapter compares against.**
    ///
    /// 3766 against 7516 is the factor of two the 30 us requirement was
    /// short by, which is the chapter's argument, so it is pinned.
    #[test]
    fn the_software_figure_is_half() {
        let software = design(ChainSpec {
            max_group_delay_s: 0.0,
            pipelined_combs: false,
            ..lock_in()
        })
        .expect("designable");
        assert_eq!(software.group_delay.total(), 3766.5);
    }
}
