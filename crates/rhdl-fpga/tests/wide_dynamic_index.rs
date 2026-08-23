//! A wide dynamic array index must not panic the compiler.
//!
//! # What was wrong
//!
//! `path_star_with_index_tracking` enumerates the concrete paths a
//! dynamic index can take, bounded by how many distinct values the index
//! can hold:
//!
//! ```rust,ignore
//! let upper_limit = array.size.min(1 << slot_bits);
//! ```
//!
//! At `slot_bits = 64` the shift overflows **before** `.min()` can clamp
//! it, so the compiler panicked with `attempt to shift left with
//! overflow` and no diagnostic at all.
//!
//! The boundary was sharp and, until measured, misleading:
//!
//! | index width | before |
//! |---|---|
//! | `b2`, `b8`, `b32`, `b63` | compile |
//! | `b64` | **panic** |
//!
//! So wide indices were always intended to work — they are clamped by
//! `.min(array.size)`. Sixty-four is purely an arithmetic edge case, not
//! a design boundary, which is why saturating the shift is the fix
//! rather than a workaround: a slot that wide can already address every
//! element of any array, so `.min()` was always going to pick
//! `array.size`.
//!
//! # Provenance
//!
//! Reported as a follow-up by the `dsp::cordic` work, which hit it while
//! indexing its arctangent table and worked around it by passing the
//! table entry as a value. That widget does not need this fix — its
//! pipeline is unrolled — but the ICE it found is real for anyone using
//! a wide index.
//!
//! **The related follow-up is not addressed and its stated dependency
//! was wrong.** "Iteration count is fixed at 16; making it a const
//! generic needs the dynamic-index bug fixed first" couples two
//! unrelated problems. A const-generic loop bound needs `ATAN_TABLE[i]`
//! with a loop variable, which produces a *type* error about index
//! sizing, not this overflow. Fixing this does not unblock that.

use rhdl::core::CompilationMode;
use rhdl::core::compiler::driver::compile_design;
use rhdl::prelude::*;

mod k {
    use rhdl::prelude::*;

    #[kernel]
    #[doc(hidden)]
    pub fn idx2(arr: [b8; 4], idx: b2) -> b8 {
        arr[idx]
    }
    #[kernel]
    #[doc(hidden)]
    pub fn idx8(arr: [b8; 4], idx: b8) -> b8 {
        arr[idx]
    }
    #[kernel]
    #[doc(hidden)]
    pub fn idx63(arr: [b8; 4], idx: Bits<63>) -> b8 {
        arr[idx]
    }
    /// The case that panicked.
    #[kernel]
    #[doc(hidden)]
    pub fn idx64(arr: [b8; 4], idx: b64) -> b8 {
        arr[idx]
    }
    /// And the widest index RHDL has, to check 64 was not merely the
    /// first of a broken range.
    #[kernel]
    #[doc(hidden)]
    pub fn idx128(arr: [b8; 4], idx: b128) -> b8 {
        arr[idx]
    }
}

/// Compiling must not panic, at any index width.
///
/// `catch_unwind` rather than a plain call: the failure mode being
/// guarded against is a **panic**, not an `Err`, and a test that only
/// unwrapped a `Result` would abort rather than report.
fn compiles_without_panicking<K: DigitalFn>(label: &str) {
    let outcome =
        std::panic::catch_unwind(|| compile_design::<K>(CompilationMode::Synchronous).is_ok());
    match outcome {
        Ok(true) => {}
        Ok(false) => panic!("{label}: compilation failed"),
        Err(_) => panic!("{label}: the compiler panicked"),
    }
}

#[test]
fn a_sixty_four_bit_index_does_not_panic_the_compiler() {
    compiles_without_panicking::<k::idx64>("b64 index");
}

#[test]
fn the_widest_index_does_not_panic_either() {
    compiles_without_panicking::<k::idx128>("b128 index");
}

/// The widths that already worked still do.
#[test]
fn narrower_indices_are_unaffected() {
    compiles_without_panicking::<k::idx2>("b2 index");
    compiles_without_panicking::<k::idx8>("b8 index");
    compiles_without_panicking::<k::idx63>("b63 index");
}

/// **A wide index selects the right element**, not merely compiles.
///
/// The fix changes how many concrete paths the lowering enumerates, so
/// "it stopped panicking" is not enough — a wrong `upper_limit` would
/// silently drop elements from the generated mux. `b63` is the control:
/// it took the same clamped path before the fix and must still agree.
///
/// **Not a catching test.** Calling a kernel directly as a Rust function
/// never invokes the compiler, so this passes with the fix reverted too
/// — the trap CLAUDE.md §4 names as "direct Rust calls to a kernel are
/// more permissive than the kernel VM". It guards the value; the
/// `iverilog` round-trip below guards the lowering.
#[test]
fn a_wide_index_selects_the_right_element() {
    let arr = [bits::<8>(10), bits::<8>(20), bits::<8>(30), bits::<8>(40)];
    for (i, expected) in [10u128, 20, 30, 40].into_iter().enumerate() {
        let wide = k::idx64(arr, bits::<64>(i as u128));
        let control = k::idx63(arr, bits::<63>(i as u128));
        assert_eq!(wide.raw(), expected, "b64 index {i}");
        assert_eq!(control.raw(), expected, "b63 index {i}");
    }
}

/// And it survives `iverilog`, which is where a mis-sized mux would show
/// up as a mismatch rather than a wrong Rust answer.
mod widget {
    use rhdl::prelude::*;
    use rhdl_fpga::core::dff;

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    pub struct WideSelect {
        out: dff::DFF<b8>,
    }

    #[derive(PartialEq, Clone, Copy, Debug, Digital)]
    pub struct In {
        pub arr: [b8; 4],
        pub idx: b64,
    }

    impl SynchronousIO for WideSelect {
        type I = In;
        type O = b8;
        type Kernel = wide_select_kernel;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn wide_select_kernel(_cr: ClockReset, i: In, q: Q) -> (b8, D) {
        let mut d = D::dont_care();
        d.out = i.arr[i.idx];
        (q.out, d)
    }
}

#[test]
fn a_wide_index_round_trips_through_iverilog() -> miette::Result<()> {
    let uut = widget::WideSelect::default();
    let arr = [bits::<8>(10), bits::<8>(20), bits::<8>(30), bits::<8>(40)];
    let seq: Vec<widget::In> = (0..8)
        .map(|k| widget::In {
            arr,
            idx: bits::<64>((k % 4) as u128),
        })
        .collect();
    let tb = uut
        .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&uut, &Default::default())?.run_iverilog()?;
    Ok(())
}
