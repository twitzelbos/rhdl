//! Memory regions: the table a receiver checks an incoming R_Key
//! against, and the translation from a peer's virtual address to a
//! local one.
//!
//! An RDMA WRITE names where it wants its payload put — a 64-bit
//! virtual address in the peer's address space, plus a 32-bit R_Key
//! that is supposed to prove the peer was given permission to write
//! there. **The R_Key is the entire access-control mechanism of RDMA.**
//! A receiver that does not check it will write anywhere in its address
//! space that an arriving packet asks it to, which is a remote
//! arbitrary-write primitive handed to anything that can reach the
//! wire.
//!
//! So the check is not a nicety and it is not deferred. This module is
//! what the receive path consults before a single payload byte is
//! placed.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
            +-+MrTable+--------------+
            |                        |
  b32       |                        |  ?MemoryRegion
 +--------->| read_rkey      region +--------->
            |                        |
  ?MrWrite  |                        |  bool
 +--------->| write           valid +--------->
            +------------------------+
")]
//!
//! # Why the table is storage and the check is a separate function
//!
//! [`MrTable`] stores regions and nothing else; [`mr_validate`] is a
//! pure kernel that decides. Keeping them apart is deliberate — the
//! decision is the part that must be right, and as a pure function over
//! four arguments it can be tested exhaustively at its boundaries
//! without a clock. A table that also judged would hide that logic
//! behind a RAM's one-cycle latency.
//!
//! # Aliasing, and why a matching index is not a matching key
//!
//! The table is indexed by the R_Key's low `N` bits, so R_Keys
//! `0x0000_0001` and `0x1000_0001` land in the same slot of a 16-entry
//! table. The stored region therefore carries its **full** R_Key and
//! [`mr_validate`] compares it, returning
//! [`MrVerdict::RkeyMismatch`] when the slot is occupied by a different
//! key.
//!
//! Without that comparison the index collision *is* the vulnerability:
//! a peer that knows any R_Key sharing the low bits of a real one would
//! be granted the real one's region. This is the same reasoning as
//! `qp::table`'s QPN aliasing note, but the consequence is sharper,
//! because here the thing being aliased is permission to write memory.
//!
//! # Example
//!
//!```
#![doc = include_str!("../../examples/mr_table.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/mr_table.md")]

use rhdl::prelude::*;
use rhdl_fpga::core::ram::synchronous::{SyncBRAM, Write};

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// A registered memory region: what a peer may write, and where it
/// actually lands locally.
pub struct MemoryRegion {
    /// The R_Key a peer must present. Stored in full, because the table
    /// is indexed by only part of it — see the module docs on aliasing.
    pub rkey: Bits<32>,
    /// The first virtual address the region covers, in the address
    /// space the *peer* was told about.
    pub va_base: Bits<64>,
    /// The region's length in bytes.
    pub length: Bits<32>,
    /// The local address the region's first byte lives at. The
    /// translation is `local_base + (va - va_base)`.
    ///
    /// Separate from `va_base` so the virtual address a peer uses need
    /// not be a local one. A host registering a buffer reports the
    /// address *it* sees; where those bytes land on this device is this
    /// device's business.
    pub local_base: Bits<32>,
    /// Whether remote writes are permitted.
    ///
    /// Present and checked even though this crate implements only
    /// WRITE, so that registering a read-only region is expressible
    /// now rather than being a field added later — which
    /// `package-manager-architecture.md` §4 would make a MAJOR version
    /// bump.
    pub remote_write: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// A request to register or deregister a region.
pub struct MrWrite {
    /// The R_Key whose slot is written. Taken from the key rather than
    /// the region so a deregistration need not restate the region.
    pub rkey: Bits<32>,
    /// The region, or `None` to deregister.
    pub region: Option<MemoryRegion>,
}

#[kernel]
/// The table index for an R_Key: its low `N` bits.
///
/// Distinct R_Keys can collide here. That is why [`mr_validate`]
/// compares the stored region's full key — see the module docs.
pub fn rkey_index<const N: usize>(rkey: Bits<32>) -> Bits<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    rkey.resize::<N>()
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// Why an access was or was not permitted.
///
/// Reasons rather than a `bool` for the same purpose as
/// [`crate::rx::parse::RxVerdict`]: on a receive path these are the
/// only diagnostic available, and "the peer used a stale R_Key" and
/// "the peer asked to write past the end of the buffer" call for
/// entirely different responses from an operator.
pub enum MrVerdict {
    #[default]
    /// The R_Key is valid, the region permits remote writes, and the
    /// whole requested range lies inside it.
    Accept,
    /// Nothing is registered in that slot.
    NoRegion,
    /// The slot holds a region, but under a different R_Key. See the
    /// module docs on aliasing — this is the check that makes indexing
    /// by low bits safe.
    RkeyMismatch,
    /// The region exists but does not permit remote writes.
    NoRemoteWrite,
    /// The requested range is not wholly inside the region.
    OutOfRange,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// The result of validating an access against a region.
pub struct MrLookup {
    /// Whether the access is permitted, and if not, why.
    pub verdict: MrVerdict,
    /// Where the first byte lands locally. Meaningful only when
    /// `verdict` is [`MrVerdict::Accept`]; zero otherwise, so that a
    /// consumer which ignores the verdict writes to address zero rather
    /// than somewhere derived from an unvalidated address.
    pub local_addr: Bits<32>,
}

#[kernel]
/// Decide whether an access is permitted, and translate it.
///
/// The range test is `[va, va + len)` wholly inside
/// `[va_base, va_base + length)`, which is the test that matters: a
/// check of only the *start* address would permit a write that begins
/// inside the region and runs off the end of it.
///
/// # On wrapping
///
/// `va - r.va_base` is computed unconditionally, because a kernel's
/// `if`/`else` evaluates both arms — so the subtraction happens even
/// for `va < va_base`, where it wraps. That is harmless here and the
/// guard still holds: a wrapped offset is enormous, so `offset <=
/// length` fails and the access is rejected. The explicit `va >=
/// va_base` term is therefore redundant, and kept because the
/// alternative is a reader having to reconstruct that argument.
///
/// Having established `offset <= length`, both are below 2^32, so
/// `offset + len` cannot wrap in 64 bits — which is what makes the
/// second half of the range test trustworthy.
pub fn mr_validate(
    region: Option<MemoryRegion>,
    rkey: Bits<32>,
    va: Bits<64>,
    len: Bits<32>,
) -> MrLookup {
    let mut out = MrLookup::dont_care();
    out.verdict = MrVerdict::Accept;
    out.local_addr = bits(0);

    match region {
        None => {
            out.verdict = MrVerdict::NoRegion;
        }
        Some(r) => {
            let offset = va - r.va_base;
            let len64 = len.resize::<64>();
            let length64 = r.length.resize::<64>();
            let fits = va >= r.va_base && offset <= length64 && (offset + len64) <= length64;

            if r.rkey != rkey {
                out.verdict = MrVerdict::RkeyMismatch;
            } else if !r.remote_write {
                out.verdict = MrVerdict::NoRemoteWrite;
            } else if !fits {
                out.verdict = MrVerdict::OutOfRange;
            } else {
                out.verdict = MrVerdict::Accept;
                out.local_addr = r.local_base + offset.resize::<32>();
            }
        }
    }
    out
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// A RAM-backed table of registered memory regions, `2^N` entries.
///
/// See the [module docs](self).
pub struct MrTable<const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    ram: SyncBRAM<Option<MemoryRegion>, N>,
}

impl<const N: usize> Default for MrTable<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    fn default() -> Self {
        // Every slot starts unregistered. Not `MemoryRegion::default()`:
        // a zeroed region has `remote_write` false and zero length, so
        // it would reject everything anyway -- but `None` says
        // "unregistered" and a zeroed region says "registered and
        // empty", and the verdicts a receiver reports should tell those
        // apart.
        Self {
            ram: SyncBRAM::new((0..(1 << N)).map(|i| (bits(i as u128), None))),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [`MrTable`].
pub struct In {
    /// The R_Key to look up. `region` reflects it on the **next**
    /// cycle, as for any synchronous RAM.
    pub read_rkey: Bits<32>,
    /// Register or deregister a region, or `None` for no write.
    pub write: Option<MrWrite>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [`MrTable`].
pub struct Out {
    /// The region in the slot addressed one cycle ago, or `None` if
    /// that slot has never been registered.
    ///
    /// **Not yet validated** — the stored region's R_Key may differ
    /// from the one looked up, because the index is only part of the
    /// key. Pass this to [`mr_validate`] along with the R_Key actually
    /// presented.
    pub region: Option<MemoryRegion>,
    /// Whether `region` is `Some`. Derived, not stored.
    pub valid: bool,
}

impl<const N: usize> SynchronousIO for MrTable<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    type I = In;
    type O = Out;
    type Kernel = mr_table<N>;
}

#[kernel]
/// Kernel for [`MrTable`].
pub fn mr_table<const N: usize>(cr: ClockReset, i: In, q: Q<N>) -> (Out, D<N>)
where
    rhdl::bits::W<N>: BitWidth,
{
    let mut d = D::<N>::dont_care();
    let mut o = Out::dont_care();

    d.ram.read_addr = rkey_index::<N>(i.read_rkey);

    let mut w = Write::<Option<MemoryRegion>, N>::dont_care();
    match i.write {
        Some(req) => {
            w.addr = rkey_index::<N>(req.rkey);
            w.value = req.region;
            w.enable = true;
        }
        None => {
            w.addr = bits(0);
            w.value = None;
            w.enable = false;
        }
    }
    d.ram.write = w;

    o.region = q.ram;
    o.valid = match q.ram {
        Some(_r) => true,
        None => false,
    };

    if cr.reset.any() {
        d.ram.read_addr = bits(0);
        d.ram.write.addr = bits(0);
        d.ram.write.value = None;
        d.ram.write.enable = false;
        o.region = None;
        o.valid = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;

    const RKEY: u128 = 0x0000_A5A5;
    const VA: u128 = 0x0000_7F00_1234_0000;
    const LEN: u128 = 4096;
    const LOCAL: u128 = 0x0001_0000;

    fn region() -> MemoryRegion {
        MemoryRegion {
            rkey: bits(RKEY),
            va_base: bits(VA),
            length: bits(LEN),
            local_base: bits(LOCAL),
            remote_write: true,
        }
    }

    fn check(va: u128, len: u128) -> MrLookup {
        mr_validate(Some(region()), bits(RKEY), bits(va), bits(len))
    }

    // ------------------------------------------- Tier 1: the decision

    #[test]
    fn an_access_inside_the_region_is_accepted_and_translated() {
        let r = check(VA + 256, 64);
        assert_eq!(r.verdict, MrVerdict::Accept);
        assert_eq!(
            r.local_addr.raw(),
            LOCAL + 256,
            "local_base + (va - va_base)"
        );
    }

    /// **The range test is on the whole range, not just its start.**
    /// Every boundary named individually, because an off-by-one here is
    /// a one-byte buffer overflow driven by a remote peer.
    #[test]
    fn the_range_test_covers_the_whole_access() {
        // Exactly the whole region.
        assert_eq!(check(VA, LEN).verdict, MrVerdict::Accept, "exact fit");
        // Last byte of the region.
        assert_eq!(
            check(VA + LEN - 1, 1).verdict,
            MrVerdict::Accept,
            "last byte"
        );
        // One byte past the end -- starts inside, ends outside. A check
        // of only the start address would accept this.
        assert_eq!(
            check(VA + LEN - 1, 2).verdict,
            MrVerdict::OutOfRange,
            "starts inside, ends one byte past"
        );
        assert_eq!(
            check(VA + 1, LEN).verdict,
            MrVerdict::OutOfRange,
            "whole length but offset by one"
        );
        // Starting exactly at the end is out of range for any non-zero
        // length.
        assert_eq!(
            check(VA + LEN, 1).verdict,
            MrVerdict::OutOfRange,
            "starts at the first byte past the region"
        );
        // Below the base.
        assert_eq!(check(VA - 1, 1).verdict, MrVerdict::OutOfRange, "one below");
        assert_eq!(check(0, 1).verdict, MrVerdict::OutOfRange, "address zero");
        // A zero-length access at the region's end is vacuously inside.
        assert_eq!(
            check(VA + LEN, 0).verdict,
            MrVerdict::Accept,
            "zero-length at the end touches nothing"
        );
    }

    /// **The aliasing check.** The table is indexed by the R_Key's low
    /// bits, so this is what stops a peer presenting any key that
    /// shares them.
    #[test]
    fn a_different_rkey_in_the_same_slot_is_rejected() {
        // Would index to the same slot in a table of up to 16 bits.
        for other in [0x1000_A5A5u128, 0xFFFF_A5A5, 0x0001_A5A5, 0] {
            if other == RKEY {
                continue;
            }
            let r = mr_validate(Some(region()), bits(other), bits(VA), bits(64));
            assert_eq!(
                r.verdict,
                MrVerdict::RkeyMismatch,
                "R_Key {other:#010x} must not be granted the region \
                 registered under {RKEY:#010x}"
            );
            assert_eq!(
                r.local_addr.raw(),
                0,
                "and a rejected access must not report an address"
            );
        }
        // The low bits really do collide, which is the premise.
        assert_eq!(
            rkey_index::<4>(bits(0x1000_A5A5)).raw(),
            rkey_index::<4>(bits(RKEY)).raw(),
            "the premise of this test is that these alias"
        );
    }

    #[test]
    fn an_unregistered_slot_and_a_read_only_region_are_both_rejected() {
        let r = mr_validate(None, bits(RKEY), bits(VA), bits(64));
        assert_eq!(r.verdict, MrVerdict::NoRegion);
        assert_eq!(r.local_addr.raw(), 0);

        let ro = MemoryRegion {
            remote_write: false,
            ..region()
        };
        let r = mr_validate(Some(ro), bits(RKEY), bits(VA), bits(64));
        assert_eq!(r.verdict, MrVerdict::NoRemoteWrite);
        assert_eq!(r.local_addr.raw(), 0);
    }

    /// Precedence matters: a wrong R_Key on a read-only region out of
    /// range should report the R_Key, because that is the most
    /// informative failure and the others are then moot.
    #[test]
    fn the_rkey_check_comes_before_the_others() {
        let ro = MemoryRegion {
            remote_write: false,
            ..region()
        };
        let r = mr_validate(Some(ro), bits(0xDEAD_BEEF), bits(0), bits(99999));
        assert_eq!(r.verdict, MrVerdict::RkeyMismatch);
    }

    /// A region at the very top of the address space must not be made
    /// accessible by arithmetic that wraps.
    #[test]
    fn a_region_near_the_top_of_the_address_space_does_not_wrap() {
        let top = MemoryRegion {
            rkey: bits(RKEY),
            va_base: bits(0xFFFF_FFFF_FFFF_F000),
            length: bits(4096),
            local_base: bits(0),
            remote_write: true,
        };
        let v =
            |va: u128, len: u128| mr_validate(Some(top), bits(RKEY), bits(va), bits(len)).verdict;
        assert_eq!(v(0xFFFF_FFFF_FFFF_F000, 4096), MrVerdict::Accept, "exact");
        assert_eq!(
            v(0xFFFF_FFFF_FFFF_FFFF, 1),
            MrVerdict::Accept,
            "the very last byte"
        );
        // An address below the base, which subtracts to a huge offset.
        assert_eq!(
            v(0, 1),
            MrVerdict::OutOfRange,
            "zero is below the base; the wrapped offset must not pass"
        );
        assert_eq!(
            v(0xFFFF_FFFF_FFFF_EFFF, 1),
            MrVerdict::OutOfRange,
            "one below the base"
        );
    }

    /// A sweep over offsets and lengths, comparing against the
    /// arithmetic written out independently in host `u128`. Cheap, and
    /// it covers the interior rather than only the boundaries above.
    #[test]
    fn validation_agrees_with_the_arithmetic_over_a_sweep() {
        for off in (0u128..5000).step_by(7) {
            for len in [0u128, 1, 63, 64, 1024, 4096] {
                let got = check(VA + off, len);
                let want_ok = off + len <= LEN;
                assert_eq!(
                    got.verdict == MrVerdict::Accept,
                    want_ok,
                    "va_base+{off}, len {len}: {off}+{len} <= {LEN} is {want_ok}"
                );
                if want_ok {
                    assert_eq!(got.local_addr.raw(), LOCAL + off);
                }
            }
        }
    }

    #[test]
    fn mr_validate_kernel_vm_and_verilog() -> miette::Result<()> {
        let mut inputs = Vec::new();
        for off in [0u128, 1, 255, 4095, 4096, 4097] {
            for len in [0u128, 1, 64, 4096] {
                inputs.push((
                    Some(region()),
                    bits::<32>(RKEY),
                    bits::<64>(VA + off),
                    bits::<32>(len),
                ));
                inputs.push((
                    Some(region()),
                    bits::<32>(0xDEAD_BEEF),
                    bits::<64>(VA + off),
                    bits::<32>(len),
                ));
            }
        }
        inputs.push((None, bits(RKEY), bits(VA), bits(1)));
        test_kernel_vm_and_verilog_synchronous::<mr_validate, _, _, _>(
            mr_validate,
            inputs.into_iter(),
        )?;
        Ok(())
    }

    #[test]
    fn rkey_index_kernel_vm_and_verilog() -> miette::Result<()> {
        let inputs = (0u128..64).map(|n| (bits::<32>(n * 1013),));
        test_kernel_vm_and_verilog_synchronous::<rkey_index<4>, _, _, _>(rkey_index::<4>, inputs)?;
        Ok(())
    }

    // ------------------------------------- Tier 2: the table over time

    fn drive(stream: Vec<In>) -> Vec<Out> {
        let uut: MrTable<4> = MrTable::default();
        let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
        uut.run(input)
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    #[test]
    fn a_registered_region_reads_back_and_a_deregistered_one_does_not() {
        let idle = In {
            read_rkey: bits(0),
            write: None,
        };
        let out = drive(vec![
            In {
                read_rkey: bits(RKEY),
                write: Some(MrWrite {
                    rkey: bits(RKEY),
                    region: Some(region()),
                }),
            },
            In {
                read_rkey: bits(RKEY),
                ..idle
            },
            In {
                read_rkey: bits(RKEY),
                write: Some(MrWrite {
                    rkey: bits(RKEY),
                    region: None,
                }),
            },
            In {
                read_rkey: bits(RKEY),
                ..idle
            },
            idle,
        ]);
        // `with_reset(1)` means output sample `m` reflects input `m-1`,
        // and the RAM adds one more cycle, so the read issued by input
        // `j` appears at sample `j + 2`.
        assert_eq!(out[3].region, Some(region()), "registered and read back");
        assert!(out[3].valid);
        assert_eq!(out[5].region, None, "deregistered");
        assert!(!out[5].valid);
    }

    #[test]
    fn every_slot_starts_unregistered() {
        let out = drive(
            (0..16u128)
                .map(|k| In {
                    read_rkey: bits(k),
                    write: None,
                })
                .collect(),
        );
        assert!(
            out.iter().all(|o| o.region.is_none() && !o.valid),
            "a fresh table must grant nothing: {out:?}"
        );
    }

    /// Two regions whose R_Keys alias share one slot, so the second
    /// registration displaces the first. The table does not prevent
    /// this -- it is a direct-mapped table -- and [`mr_validate`] is
    /// what makes the consequence safe: the displaced key is then
    /// rejected rather than granted the wrong region.
    #[test]
    fn aliasing_registrations_displace_and_the_check_catches_it() {
        let other = MemoryRegion {
            rkey: bits(0x1000_A5A5),
            va_base: bits(0x1000),
            length: bits(16),
            local_base: bits(0x9000),
            remote_write: true,
        };
        let idle = In {
            read_rkey: bits(0),
            write: None,
        };
        let out = drive(vec![
            In {
                read_rkey: bits(0),
                write: Some(MrWrite {
                    rkey: bits(RKEY),
                    region: Some(region()),
                }),
            },
            In {
                read_rkey: bits(0),
                write: Some(MrWrite {
                    rkey: bits(0x1000_A5A5),
                    region: Some(other),
                }),
            },
            In {
                read_rkey: bits(RKEY),
                ..idle
            },
            idle,
            idle,
        ]);
        let stored = out[4].region.expect("the slot holds the second region");
        assert_eq!(stored, other, "the second registration displaced the first");
        // And the original key is now refused rather than being handed
        // the wrong region.
        let r = mr_validate(Some(stored), bits(RKEY), bits(VA), bits(64));
        assert_eq!(r.verdict, MrVerdict::RkeyMismatch);
    }

    #[test]
    fn reset_clears_the_outputs() {
        let cr = clock_reset(clock(false), reset(true));
        let q = Q::<4> {
            ram: Some(region()),
        };
        let i = In {
            read_rkey: bits(RKEY),
            write: Some(MrWrite {
                rkey: bits(RKEY),
                region: Some(region()),
            }),
        };
        let (o, d) = mr_table::<4>(cr, i, q);
        assert_eq!(o.region, None, "no region is reported under reset");
        assert!(!o.valid);
        assert!(!d.ram.write.enable, "and no write is issued");
    }

    // ------------------------------------------ Tiers 3, 4 and 5

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut: MrTable<4> = MrTable::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        assert!(hdl.contains("module top"));
        Ok(())
    }

    fn test_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In)>> {
        let idle = In {
            read_rkey: bits(0),
            write: None,
        };
        vec![
            In {
                read_rkey: bits(RKEY),
                write: Some(MrWrite {
                    rkey: bits(RKEY),
                    region: Some(region()),
                }),
            },
            In {
                read_rkey: bits(RKEY),
                ..idle
            },
            In {
                read_rkey: bits(0x1000_A5A5),
                ..idle
            },
            idle,
            idle,
        ]
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100)
    }

    #[test]
    fn test_widget_hdl_works() -> miette::Result<()> {
        let uut: MrTable<4> = MrTable::default();
        let tb = uut
            .run(test_stream())
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_widget_trace() -> miette::Result<()> {
        let uut: MrTable<4> = MrTable::default();
        let vcd = uut.run(test_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("mr_table");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["66ede010294a13ae44a6fadb1402e1e2e8e094961c8f6547acbedd81d2ec5ef6"];
        let digest = vcd.dump_to_file(root.join("mr_table.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
