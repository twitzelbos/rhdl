//! A widget may be generic over a sub-circuit.
//!
//! Before the DQ derives emitted their value traits with bounds on
//! field types rather than type parameters, this file did not compile.
//! `#[derive(Copy)]` on the generated `Q<C>` demanded `C: Copy` of a
//! *circuit*, even though `C` does not appear in any field type once
//! `<C as SynchronousIO>::O` is normalised.
//!
//! The tests instantiate the same generic widget with two genuinely
//! different sub-circuits and check that the composed behaviour tracks
//! the sub-circuit. Compiling is necessary but not sufficient: a
//! version that erased the sub-circuit would also compile.

use rhdl::prelude::*;
use rhdl_fpga::core::{delay::Delay, dff};

/// A sub-circuit followed by one register.
///
/// The sub-circuit's interface is pinned by the bound, so nothing in
/// the kernel is generic — `q.inner` is a `bool` whatever `C` is. That
/// is the case the derives were getting wrong.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct Pipe<C>
where
    C: SynchronousIO<I = bool, O = bool> + Synchronous + Default + Clone + std::fmt::Debug,
{
    inner: C,
    tail: dff::DFF<bool>,
}

impl<C> SynchronousIO for Pipe<C>
where
    C: SynchronousIO<I = bool, O = bool> + Synchronous + Default + Clone + std::fmt::Debug,
{
    type I = bool;
    type O = bool;
    type Kernel = pipe_kernel<C>;
}

#[kernel]
#[doc(hidden)]
pub fn pipe_kernel<C>(cr: ClockReset, i: bool, q: Q<C>) -> (bool, D<C>)
where
    C: SynchronousIO<I = bool, O = bool> + Synchronous + Default + Clone + std::fmt::Debug,
{
    let mut d = D::<C>::dont_care();
    d.inner = i;
    d.tail = q.inner;
    let mut o = q.tail;
    if cr.reset.any() {
        d.inner = false;
        d.tail = false;
        o = false;
    }
    (o, d)
}

/// A single pulse, then quiet — so the output's position is the
/// composed latency and nothing else.
fn pulse() -> impl Iterator<Item = TimedSample<(ClockReset, bool)>> {
    (0..24)
        .map(|k| k == 2)
        .collect::<Vec<_>>()
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100)
}

fn latency<C>(uut: Pipe<C>) -> usize
where
    C: SynchronousIO<I = bool, O = bool> + Synchronous + Default + Clone + std::fmt::Debug,
{
    let out: Vec<bool> = uut
        .run(pulse())
        .synchronous_sample()
        .map(|t| t.output)
        .collect();
    let high: Vec<usize> = out
        .iter()
        .enumerate()
        .filter(|(_, v)| **v)
        .map(|(k, _)| k)
        .collect();
    assert_eq!(high.len(), 1, "one pulse in, one pulse out: {high:?}");
    high[0]
}

#[test]
fn the_sub_circuit_is_not_erased() {
    // `DFF` is one cycle, `Delay<_, 3>` is three. The wrapper adds its
    // own register to both. If the generic were erased -- if `Pipe`
    // somehow collapsed to one implementation -- these would agree.
    let with_dff = latency(Pipe::<dff::DFF<bool>>::default());
    let with_delay = latency(Pipe::<Delay<bool, 3>>::default());
    assert_eq!(
        with_delay - with_dff,
        2,
        "Delay<_,3> is two cycles longer than a DFF: {with_dff} vs {with_delay}"
    );
}

#[test]
fn it_emits_hdl_for_each_instantiation() -> miette::Result<()> {
    let a = Pipe::<dff::DFF<bool>>::default();
    let b = Pipe::<Delay<bool, 3>>::default();
    let ha = a.descriptor("top".into())?.hdl()?.modules.pretty();
    let hb = b.descriptor("top".into())?.hdl()?.modules.pretty();
    assert!(!ha.is_empty() && !hb.is_empty());
    assert_ne!(
        ha, hb,
        "two different sub-circuits must emit two different modules"
    );
    Ok(())
}

#[test]
fn iverilog_agrees_with_the_simulator() -> miette::Result<()> {
    // The ground truth: the composed widget is real hardware, not just
    // a type that type-checks.
    let uut = Pipe::<Delay<bool, 3>>::default();
    let tb = uut.run(pulse()).collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&uut, &Default::default())?.run_iverilog()?;
    Ok(())
}

/// The asynchronous half of the change.
///
/// `CircuitDQ` had the identical defect and got the identical fix, so
/// it needs its own coverage — the two derives are separate code paths
/// that happen to look alike.
mod asynchronous {
    use super::*;
    use rhdl_fpga::cdc::synchronizer::{In as SyncIn, Sync1Bit};

    #[derive(Debug, Clone, Default, Circuit, CircuitDQ)]
    #[rhdl(dq_no_prefix)]
    pub struct Wrapper<W: Domain, R: Domain, C>
    where
        C: CircuitIO<I = SyncIn<W, R>, O = Signal<bool, R>>
            + Circuit
            + Default
            + Clone
            + std::fmt::Debug,
    {
        inner: C,
    }

    impl<W: Domain, R: Domain, C> CircuitIO for Wrapper<W, R, C>
    where
        C: CircuitIO<I = SyncIn<W, R>, O = Signal<bool, R>>
            + Circuit
            + Default
            + Clone
            + std::fmt::Debug,
    {
        type I = SyncIn<W, R>;
        type O = Signal<bool, R>;
        type Kernel = wrapper_kernel<W, R, C>;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn wrapper_kernel<W: Domain, R: Domain, C>(
        i: SyncIn<W, R>,
        q: Q<W, R, C>,
    ) -> (Signal<bool, R>, D<W, R, C>)
    where
        C: CircuitIO<I = SyncIn<W, R>, O = Signal<bool, R>>
            + Circuit
            + Default
            + Clone
            + std::fmt::Debug,
    {
        let mut d = D::<W, R, C>::dont_care();
        d.inner = i;
        (q.inner, d)
    }

    #[test]
    fn a_generic_async_sub_circuit_elaborates() -> miette::Result<()> {
        let uut = Wrapper::<Red, Blue, Sync1Bit<Red, Blue>>::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        assert!(!hdl.is_empty());
        Ok(())
    }
}
