//! The queue-pair context table — what makes more than one connection
//! possible.
//!
//! `roce-v2-architecture.md` §6 promised a "queue-pair context table
//! (RAM-backed)" and the first implementation shipped a plain struct
//! passed in as an input, which means exactly one connection. This is
//! the table, and it is FERoCE's `RoCE_qp_state_module`'s other half.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +------+QpTable+---------+
     |                        |
B<24>|                        | ?Ctx
+--->| read_qpn           ctx +--->
     |                        |
?Wr  |                        | bool
+--->| write            valid +--->
     |                        |
     +------------------------+
")]
//!
//!# Validity is the `Option`, not a side flag
//!
//! The table stores `Option<QpContext>`, so an unconfigured entry reads
//! back as `None` and `valid` is derived from it. A separate validity
//! bitmap would be a second source of truth that can disagree with the
//! first — and the failure it would permit is the worst kind here:
//! transmitting with a stale context to a queue pair the peer has torn
//! down.
//!
//!# The read is registered, and that is the thing to know
//!
//! `ctx` arrives **one cycle after** `read_qpn` is presented, because
//! the table is block RAM. For the send path this costs nothing — the
//! queue-pair number is constant for the whole of a message, so the
//! lookup happens once at message start and is long settled by the time
//! the first header byte is emitted. It would matter if a design
//! interleaved packets from different queue pairs cycle by cycle, which
//! nothing here does.
//!
//!# Indexing
//!
//! A 24-bit queue-pair number cannot index a 16-million-entry table, so
//! the low `N` bits are used and the rest ignored. That is what every
//! implementation does, and it has a consequence worth stating: two
//! queue pairs whose numbers differ only above bit `N` **alias**. The
//! host allocates queue-pair numbers, so this is a constraint on the
//! control plane rather than a bug — but it is the sort of constraint
//! that is discovered in the field if it is not written down.
//! [`qpn_index`] is the one place it happens.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/qp_table.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/qp_table.md")]

use super::context::QpContext;
use rhdl::prelude::*;
use rhdl_fpga::core::ram::synchronous::{SyncBRAM, Write};

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// A queue-pair number paired with the context to store for it.
pub struct QpWrite {
    /// The queue pair being configured.
    pub qpn: Bits<24>,
    /// Its context, or `None` to tear the entry down.
    pub ctx: Option<QpContext>,
}

#[kernel]
/// Map a 24-bit queue-pair number to a table index.
///
/// The low `N` bits, so queue pairs differing only above bit `N` alias
/// — see the module docs.
pub fn qpn_index<const N: usize>(qpn: Bits<24>) -> Bits<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    qpn.resize::<N>()
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [`QpTable`].
pub struct In {
    /// The queue pair to look up. `ctx` reflects it on the **next**
    /// cycle.
    pub read_qpn: Bits<24>,
    /// Configure or tear down an entry, or `None` for no write.
    pub write: Option<QpWrite>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs of the [`QpTable`].
pub struct Out {
    /// The context for the queue pair presented one cycle ago, or
    /// `None` if that entry has never been configured.
    pub ctx: Option<QpContext>,
    /// Whether `ctx` is `Some`. Derived, not stored — see the module
    /// docs on why there is no separate validity bitmap.
    pub valid: bool,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// A table of `2^N` queue-pair contexts.
pub struct QpTable<const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    ram: SyncBRAM<Option<QpContext>, N>,
}

impl<const N: usize> Default for QpTable<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    fn default() -> Self {
        // Every entry starts unconfigured. Not `QpContext::default()`:
        // a zeroed context is a *plausible* connection to queue pair 0,
        // and transmitting to it would be worse than refusing to.
        Self {
            ram: SyncBRAM::new((0..(1 << N)).map(|i| (bits(i as u128), None))),
        }
    }
}

impl<const N: usize> SynchronousIO for QpTable<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    type I = In;
    type O = Out;
    type Kernel = qp_table<N>;
}

#[kernel]
/// Kernel for [`QpTable`].
pub fn qp_table<const N: usize>(cr: ClockReset, i: In, q: Q<N>) -> (Out, D<N>)
where
    rhdl::bits::W<N>: BitWidth,
{
    let mut d = D::<N>::dont_care();
    let mut o = Out::dont_care();

    d.ram.read_addr = qpn_index::<N>(i.read_qpn);

    let mut w = Write::<Option<QpContext>, N>::dont_care();
    match i.write {
        Some(req) => {
            w.addr = qpn_index::<N>(req.qpn);
            w.value = req.ctx;
            w.enable = true;
        }
        None => {
            w.addr = bits(0);
            w.value = None;
            w.enable = false;
        }
    }
    d.ram.write = w;

    o.ctx = q.ram;
    o.valid = match q.ram {
        Some(_c) => true,
        None => false,
    };

    if cr.reset.any() {
        d.ram.read_addr = bits(0);
        d.ram.write.addr = bits(0);
        d.ram.write.value = None;
        d.ram.write.enable = false;
        o.ctx = None;
        o.valid = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::Transport;
    use crate::qp::context::Pmtu;
    use expect_test::expect;
    use std::path::PathBuf;

    type Uut = QpTable<4>; // 16 entries

    fn ctx(n: u128) -> QpContext {
        QpContext {
            dqpn: bits(0x1000 + n),
            transport: if n.is_multiple_of(2) {
                Transport::Uc
            } else {
                Transport::Rc
            },
            pmtu: Pmtu::B1024,
            rkey: bits(0xA000_0000 + n),
            pkey: bits(0xFFFF),
        }
    }

    fn idle() -> In {
        In {
            read_qpn: bits(0),
            write: None,
        }
    }

    /// Run a stimulus and return the `(ctx, valid)` sample stream.
    fn run(stream: Vec<In>) -> Vec<(Option<QpContext>, bool)> {
        let uut = Uut::default();
        uut.run(stream.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| (s.output.ctx, s.output.valid))
            .collect()
    }

    // ---------------- Tier 1 ----------------

    /// An unconfigured entry reads back as `None`, not as a zeroed
    /// context. A zeroed context is a plausible-looking connection to
    /// queue pair 0, and transmitting to it is worse than refusing to.
    #[test]
    fn an_unconfigured_entry_is_none() {
        let mut stream = vec![idle()];
        for qpn in [0u128, 1, 7, 15, 0x4242] {
            stream.push(In {
                read_qpn: bits(qpn),
                write: None,
            });
            stream.push(idle());
        }
        let out = run(stream);
        assert!(
            out.iter().all(|(c, v)| c.is_none() && !v),
            "a fresh table must read back entirely None: {out:?}"
        );
    }

    /// `qpn_index` takes the low `N` bits, so queue pairs differing only
    /// above bit `N` alias. Documented behaviour, pinned so it cannot
    /// change silently.
    #[test]
    fn the_index_is_the_low_bits_and_aliasing_is_explicit() {
        assert_eq!(qpn_index::<4>(bits(0x000005)), bits::<4>(5));
        assert_eq!(qpn_index::<4>(bits(0x123455)), bits::<4>(5));
        assert_eq!(
            qpn_index::<4>(bits(0x000005)),
            qpn_index::<4>(bits(0xFFFFF5)),
            "queue pairs differing only above bit N alias, by design"
        );
        assert_eq!(qpn_index::<8>(bits(0x001234)), bits::<8>(0x34));
    }

    #[test]
    fn a_write_request_drives_the_ram_write_port() {
        let cr = ClockReset::dont_care();
        let q = Q::<4> { ram: None };
        let (_o, d) = qp_table::<4>(
            cr,
            In {
                read_qpn: bits(3),
                write: Some(QpWrite {
                    qpn: bits(0x000007),
                    ctx: Some(ctx(7)),
                }),
            },
            q,
        );
        assert!(d.ram.write.enable);
        assert_eq!(d.ram.write.addr, bits::<4>(7));
        assert_eq!(d.ram.write.value, Some(ctx(7)));
        assert_eq!(
            d.ram.read_addr,
            bits::<4>(3),
            "the read port is independent"
        );
    }

    #[test]
    fn no_write_request_leaves_the_write_port_disabled() {
        let cr = ClockReset::dont_care();
        let q = Q::<4> { ram: Some(ctx(1)) };
        let (o, d) = qp_table::<4>(cr, idle(), q);
        assert!(!d.ram.write.enable);
        assert_eq!(o.ctx, Some(ctx(1)));
        assert!(o.valid);
    }

    /// A tear-down writes `None`, which is how an entry is invalidated
    /// without a separate bitmap.
    #[test]
    fn a_teardown_writes_none() {
        let cr = ClockReset::dont_care();
        let q = Q::<4> { ram: Some(ctx(2)) };
        let (_o, d) = qp_table::<4>(
            cr,
            In {
                read_qpn: bits(2),
                write: Some(QpWrite {
                    qpn: bits(2),
                    ctx: None,
                }),
            },
            q,
        );
        assert!(d.ram.write.enable);
        assert_eq!(d.ram.write.value, None);
    }

    #[test]
    fn reset_disables_the_write_and_reports_invalid() {
        let cr = clock_reset(clock(false), reset(true));
        let q = Q::<4> { ram: Some(ctx(1)) };
        let (o, d) = qp_table::<4>(
            cr,
            In {
                read_qpn: bits(9),
                write: Some(QpWrite {
                    qpn: bits(9),
                    ctx: Some(ctx(9)),
                }),
            },
            q,
        );
        assert!(!d.ram.write.enable, "reset must not let a write through");
        assert_eq!(o.ctx, None);
        assert!(!o.valid);
    }

    // ---------------- Tier 2 ----------------

    /// Configure several entries, then read them back — each returns its
    /// own context, which is the whole point of having a table.
    #[test]
    fn several_queue_pairs_hold_distinct_contexts() {
        let qpns = [1u128, 2, 5, 11];
        let mut stream = Vec::new();
        for (k, qpn) in qpns.iter().enumerate() {
            stream.push(In {
                read_qpn: bits(0),
                write: Some(QpWrite {
                    qpn: bits(*qpn),
                    ctx: Some(ctx(k as u128 + 1)),
                }),
            });
        }
        // Read each back. The read is registered, so the sample for a
        // given `read_qpn` lands on the following cycle.
        let read_at = stream.len();
        for qpn in qpns.iter() {
            stream.push(In {
                read_qpn: bits(*qpn),
                write: None,
            });
        }
        stream.push(idle());
        let out = run(stream);
        for (k, _qpn) in qpns.iter().enumerate() {
            // +1 for `with_reset`, +1 for the registered read.
            let s = &out[read_at + k + 2];
            assert_eq!(
                s.0,
                Some(ctx(k as u128 + 1)),
                "queue pair index {k} read back the wrong context: {out:?}"
            );
            assert!(s.1, "and should report valid");
        }
    }

    /// An entry torn down reads back as `None` again.
    #[test]
    fn a_torn_down_entry_reads_back_invalid() {
        let mut stream = vec![
            In {
                read_qpn: bits(0),
                write: Some(QpWrite {
                    qpn: bits(6),
                    ctx: Some(ctx(6)),
                }),
            },
            In {
                read_qpn: bits(6),
                write: None,
            },
            idle(),
            idle(),
        ];
        let before = run(stream.clone());
        assert_eq!(
            before[3].0,
            Some(ctx(6)),
            "should be configured: {before:?}"
        );
        // Now tear it down and read again.
        stream.push(In {
            read_qpn: bits(0),
            write: Some(QpWrite {
                qpn: bits(6),
                ctx: None,
            }),
        });
        stream.push(In {
            read_qpn: bits(6),
            write: None,
        });
        stream.push(idle());
        stream.push(idle());
        let after = run(stream);
        let last = after.last().unwrap();
        assert_eq!(last.0, None, "after teardown it must read None: {after:?}");
        assert!(!last.1);
    }

    // ---------------- Tier 3/4/5 ----------------

    fn stimulus() -> Vec<In> {
        let mut v = Vec::new();
        for (k, qpn) in [1u128, 2, 5].iter().enumerate() {
            v.push(In {
                read_qpn: bits(0),
                write: Some(QpWrite {
                    qpn: bits(*qpn),
                    ctx: Some(ctx(k as u128 + 1)),
                }),
            });
        }
        for qpn in [1u128, 2, 5, 9] {
            v.push(In {
                read_qpn: bits(qpn),
                write: None,
            });
        }
        v.push(In {
            read_qpn: bits(0),
            write: Some(QpWrite {
                qpn: bits(2),
                ctx: None,
            }),
        });
        v.push(In {
            read_qpn: bits(2),
            write: None,
        });
        v.push(idle());
        v
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        // Memory-backed: the full text is mostly RAM initialisation, so
        // this pins the structure, as `core::ram::option_sync` does.
        assert!(hdl.contains("module top"));
        assert!(
            hdl.contains("top_ram"),
            "the RAM child should be instantiated"
        );
        let expect = expect!["true"];
        expect.assert_eq(&(hdl.matches("endmodule").count() >= 2).to_string());
        Ok(())
    }

    #[test]
    fn test_qp_table_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let input = stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        // `.skip(2)`: a block RAM's output register is `x` in Verilog
        // until the first read completes.
        let opts = TestBenchOptions::default().skip(2);
        tb.rtl(&uut, &opts)?.run_iverilog()?;
        tb.ntl(&uut, &opts)?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_qp_table_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let input = stimulus().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("qp_table");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["827efb0c48271e53cd2b031584bb0add34673215e3c66bb70e2a842160d3f1d9"];
        let digest = vcd.dump_to_file(root.join("qp_table.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
