#![warn(missing_docs)]
//! Protocol checking: the assertions, and the traffic that breaks them.
//!
//! `axi4-architecture.md` §7 sets out four validation tiers and the
//! reason there is more than one: a loopback between two halves of this
//! crate proves they agree, not that they are right. Tier A — the
//! [`monitor`] and [`read_monitor`] widgets — is the fast oracle, and it
//! is explicitly **not** independent: it is RHDL checking RHDL, so a
//! shared misreading of the spec passes. Tier B's hand-written Verilog
//! monitors, in `verilog/axi4/`, are what address that.
//!
//! # Why the violations come first
//!
//! [`traffic`] generates bus traffic that breaks each rule
//! deliberately, and it exists *before* the monitors that catch them. A
//! monitor is only worth what it rejects, and a checker written without
//! known-bad input tends to pass everything — the failure mode this
//! module is arranged to prevent.
//!
//! So the order is: name the violations, generate traffic exhibiting
//! each, then require the monitor to report exactly the one injected.
//!
//! # What that ordering actually caught
//!
//! Every assertion passed on its first run, which is the moment to
//! distrust a checker rather than to ship one. Disabling each of the
//! write monitor's fourteen checks in turn and re-running the gate
//! showed twelve were load-bearing — and that two,
//! [`rules::Fault::BValidDropped`] and [`rules::Fault::BPayloadChanged`],
//! could be **deleted outright with every test still green**. No
//! transcript had ever stalled the response channel, because the B
//! handshake binds the *subordinate* and traffic written from a
//! manager's point of view never exercises it.
//!
//! An unexercised check is worse than a missing one, because it reads
//! as coverage. A mutation run cannot be a `cargo test`, but its
//! conclusion can: [`rules::Fault::all`] exists so that
//! `every_fault_is_exercised_by_some_transcript` can require every
//! reportable fault to be reachable from some transcript, across both
//! channels.
//!
//! # And why there are two monitors per channel
//!
//! Tier B lives in `verilog/axi4/`, hand-written from the AMBA
//! specification, and `tests/axi4_tier_b_monitor.rs` requires each
//! pair to agree on **all three outputs** — verdict, fault tally and
//! unchecked-beat count — for every transcript. Agreeing on the verdict
//! alone would be satisfied by two monitors that detect the same fault
//! for different reasons on different cycles.
//!
//! That strictness has already paid: the write pair clamped an illegal
//! `AxSIZE` oppositely, so one reported four phantom strobe faults the
//! other did not, with both still naming the right verdict. Comparing
//! only verdicts would have shipped it.
//!
//! # Structure
//!
//! The two channels are separate widgets sharing one fault vocabulary
//! and one set of address-phase rules ([`rules`]). AW and AR carry the
//! same [`super::types::AxiAddress`] and the spec constrains both
//! identically, so [`rules::address_fault`] is shared as a kernel
//! rather than written twice — two copies would be two places to fix a
//! misreading, and a drift between them would leave both monitors'
//! tests passing.

pub mod monitor;
pub mod read_monitor;
pub mod rules;
pub mod traffic;

#[cfg(test)]
mod tests {
    use super::rules::Fault;
    use super::traffic::{ReadViolation, Violation};

    /// Every fault either monitor can report must be reachable from
    /// some transcript.
    ///
    /// This is the permanent form of a one-off mutation run that
    /// disabled each check in turn and asked whether the gate noticed.
    /// It noticed for twelve of fourteen: `BValidDropped` and
    /// `BPayloadChanged` could be deleted outright with every test
    /// still green, because no transcript ever stalled the response
    /// channel.
    ///
    /// It lives here rather than in either monitor because the `Fault`
    /// vocabulary is shared: a read-only fault is unreachable from the
    /// write generator and vice versa, so only the union of both
    /// generators can answer the question.
    #[test]
    fn every_fault_is_exercised_by_some_transcript() {
        let mut reachable: Vec<Fault> = Violation::all()
            .into_iter()
            .map(Violation::expected)
            .collect();
        reachable.extend(
            ReadViolation::all()
                .into_iter()
                .map(ReadViolation::expected),
        );
        let orphans: Vec<Fault> = Fault::all()
            .into_iter()
            .filter(|f| !reachable.contains(f))
            .collect();
        assert!(
            orphans.is_empty(),
            "monitors report faults no transcript provokes, so these checks never run: {orphans:?}"
        );
    }
}
