use rhdl::prelude::*;

// ANCHOR: output_example
//                                          👇 - new!
#[derive(PartialEq, Digital, Clone, Copy, Timed)]
pub struct Output {
    out1: Signal<b16, Red>,
    out2: Signal<b16, Red>,
    t_clk: Signal<Clock, Red>,
    t_out: Signal<b16, Red>,
    p_out: Signal<b16, Red>,
    bt_ready: Signal<bool, Red>,
}
// ANCHOR_END: output_example

#[cfg(feature = "timed_impl")]
// ANCHOR: timed_impl
impl Timed for Output
where
    Signal<b16, Red>: rhdl::core::Timed,
    Signal<b16, Red>: rhdl::core::Timed,
    Signal<Clock, Red>: rhdl::core::Timed,
    Signal<b16, Red>: rhdl::core::Timed,
    Signal<b16, Red>: rhdl::core::Timed,
    Signal<bool, Red>: rhdl::core::Timed,
{
}
// ANCHOR_END: timed_impl

#[cfg(feature = "timed_blanket_impl")]
// ANCHOR: timed_blanket_impl
impl Timed for Output {}
// ANCHOR_END: timed_blanket_impl

/// The trait summaries the chapter quotes.
///
/// These mirror `rhdl::core::CircuitIO` and `rhdl::core::Timed` rather
/// than re-exporting them, because the chapter wants the *shape* of each
/// trait without the doc comments and supertrait plumbing that would
/// bury it. Keeping them here as compiling code means a change to the
/// real traits that makes these summaries wrong is at least a change
/// somebody has to look at.
mod summaries {
    use super::*;

    // Quoted by `timed/derive.md`; never called.
    #[allow(dead_code)]
    // ANCHOR: circuit_io_trait
    pub trait CircuitIO: 'static + CircuitDQ {
        // The input type of the circuit
        type I: Timed;
        // The output type of the circuit
        type O: Timed;
        // The kernel: fn(I, Q) -> (O, D), annotated with #[kernel]
        type Kernel: DigitalFn + DigitalFn2<A0 = Self::I, A1 = Self::Q, O = (Self::O, Self::D)>;
    }
    // ANCHOR_END: circuit_io_trait

    // ANCHOR: timed_trait
    pub trait Timed: Digital {}
    // ANCHOR_END: timed_trait
}
