#![warn(missing_docs)]
//! Protocol checking: the assertions, and the traffic that breaks them.
//!
//! `axi4-architecture.md` §7 sets out four validation tiers and the
//! reason there is more than one: a loopback between two halves of this
//! crate proves they agree, not that they are right. Tier A — this
//! module — is the fast oracle, and it is explicitly **not**
//! independent: it is RHDL checking RHDL, so a shared misreading of the
//! spec passes. Tier B's hand-written Verilog monitor is what addresses
//! that.
//!
//! # Why the violations come first
//!
//! [`traffic`] generates bus traffic that breaks each rule
//! deliberately, and it exists *before* the monitor that catches them.
//! A monitor is only worth anything if it fails on known-bad input, and
//! a checker written without one tends to pass everything — the failure
//! mode this module is arranged to prevent.
//!
//! So the order is: name the violations, generate traffic exhibiting
//! each, then require the monitor to report exactly the one injected.
//!
//! # What that ordering actually caught
//!
//! Every assertion passed on its first run, which is the moment to
//! distrust a checker rather than to ship one. Disabling each of the
//! monitor's fourteen checks in turn and re-running the gate showed
//! twelve were load-bearing — and that two, `BValidDropped` and
//! `BPayloadChanged`, could be **deleted outright with every test
//! still green**. No transcript had ever stalled the response channel,
//! because the B handshake binds the *subordinate* and traffic written
//! from a manager's point of view never exercises it.
//!
//! An unexercised check is worse than a missing one, because it reads
//! as coverage. A mutation run cannot be a `cargo test`, but its
//! conclusion can: [`monitor::Fault::all`] and
//! [`traffic::Violation::expected`] exist so that
//! `every_fault_is_exercised_by_some_transcript` can require every
//! reportable fault to be reachable from some transcript.
//!
//! # And why there are two monitors
//!
//! Tier B lives in `verilog/axi4/axi4_write_monitor.v`, hand-written from
//! the AMBA specification, and `tests/axi4_tier_b_monitor.rs` requires
//! the two to agree on **all three outputs** — verdict, fault tally and
//! unchecked-beat count — for every transcript. Agreeing on the verdict
//! alone would be satisfied by two monitors that detect the same fault
//! for different reasons on different cycles.
//!
//! That strictness has already paid: the two clamped an illegal
//! `AxSIZE` oppositely, so one reported four phantom strobe faults the
//! other did not, with both still naming the right verdict. Comparing
//! only verdicts would have shipped it.

pub mod monitor;
pub mod traffic;
