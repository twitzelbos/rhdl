//! A complete UC receive node: connection management and data
//! reception on one wire.
//!
//! The two halves of Phase 4 wired together. Frames arrive on a single
//! byte stream, as they do on a real interface; the connection manager
//! takes the ones addressed to its port and the RDMA receiver takes
//! the ones addressed to 4791, each ignoring what is not theirs. The
//! manager's queue-pair and memory-region writes go straight into the
//! receiver's tables, so a host that has negotiated a connection can
//! immediately write to it.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
            +-+RoceUcRxNode+----------+
            |                          |
  ?u8       |                          |  ?Placement
 +--------->| data               write +--------->
            |                          |
  bool      |                          |  RxOutcome
 +--------->| sof              outcome +--------->
            |                          |
  bool      |                          |  ?u8
 +--------->| eof                reply +--------->
            |                          |
  Frame     |                          |  CmStatus
 +--------->| cfg             cm_status +-------->
            +--------------------------+
")]
//!
//! # Internals
#![doc = badascii_doc::badascii!("
    one byte stream in
          |
          +-------------------------+
          |                         |
          v                         v
  +-------+-----------+   +---------+--------------+
  | ConnectionManager |   |     RoceWriteRx        |
  |   port = cm_port  |   |  parse + ICRC + R_Key  |
  |                   |   |  + reassembly + place  |
  +--+------------+---+   +---------+--------------+
     |            |                 ^
     | reply      | qp_write        |
     v            | mr_write        |
   out            +-----------------+
")]
//!
//! # Why the fan-out is safe
//!
//! Both children see every byte of every frame, which looks like it
//! should cause trouble and does not: each rejects what is not
//! addressed to it, at the UDP destination port, and neither has any
//! effect while rejecting. A RoCE data frame makes the manager go to
//! [`crate::cm::manager::CmPhase::Drain`] and emit nothing; a
//! connection-manager datagram makes the receiver report
//! [`crate::rx::parse::RxVerdict::NotRocePort`] and place nothing.
//!
//! Demultiplexing before the two would need a third parser to read the
//! port — which is the same forty lines of Ethernet/IPv4/UDP offsets a
//! third time, for no gain. Two independent rejections cost two small
//! state machines and need no agreement between them.
//!
//! # Configuration has two doors, and the host's wins
//!
//! The receiver's tables can be written by the connection manager *or*
//! by the host directly through this widget's `qp_write` and
//! `mr_write` inputs. Both exist because they serve different needs:
//! the manager is how a remote peer negotiates, and the direct inputs
//! are how this device's own processor sets things up — including, if
//! it chooses, before any peer has spoken.
//!
//! **When both present a write in the same cycle, the direct input
//! wins.** The local processor is the higher authority: a remote peer
//! must not be able to override a configuration the host has just
//! installed. The losing write is dropped rather than queued, and
//! `cm_overridden` reports it, because silently applying a remote
//! write a cycle later would be worse than dropping it.
//!
//! # Example
//!
//!```
#![doc = include_str!("../../examples/cm_node.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/cm_node.md")]

use super::manager::{CmPhase, ConnectionManager};
use super::message::CmStatus;
use crate::frame::FrameConfig;
use crate::qp::mr::MrWrite;
use crate::qp::table::QpWrite;
use crate::rx::endpoint::{Placement, RoceWriteRx, RxOutcome};
use crate::rx::parse::RxHeaders;
use rhdl::prelude::*;

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// A UC RDMA WRITE receive node with connection management.
///
/// See the [module docs](self). `QPN_W` and `RKEY_W` size the
/// queue-pair and memory-region tables, as in
/// [`crate::rx::endpoint::RoceWriteRx`].
pub struct RoceUcRxNode<const QPN_W: usize, const RKEY_W: usize>
where
    rhdl::bits::W<QPN_W>: BitWidth,
    rhdl::bits::W<RKEY_W>: BitWidth,
{
    cm: ConnectionManager,
    rx: RoceWriteRx<QPN_W, RKEY_W>,
}

impl<const QPN_W: usize, const RKEY_W: usize> Default for RoceUcRxNode<QPN_W, RKEY_W>
where
    rhdl::bits::W<QPN_W>: BitWidth,
    rhdl::bits::W<RKEY_W>: BitWidth,
{
    fn default() -> Self {
        Self {
            cm: ConnectionManager::default(),
            rx: RoceWriteRx::default(),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [`RoceUcRxNode`].
pub struct In {
    /// The next frame byte, or `None` when the link is idle. Every
    /// frame goes to both children.
    pub data: Option<Bits<8>>,
    /// This byte is the first of a frame.
    pub start_of_frame: bool,
    /// This byte is the last of a frame.
    pub end_of_frame: bool,
    /// This interface's MAC and IPv4 address, for the connection
    /// manager's replies.
    pub cfg: FrameConfig,
    /// The UDP port the connection manager listens on.
    pub cm_port: Bits<16>,
    /// The reply sink can accept a byte this cycle.
    pub reply_ready: bool,
    /// A direct queue-pair write from the local processor. Takes
    /// precedence over the connection manager — see the module docs.
    pub qp_write: Option<QpWrite>,
    /// A direct memory-region write from the local processor. Likewise.
    pub mr_write: Option<MrWrite>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [`RoceUcRxNode`].
pub struct Out {
    /// A payload byte and the local address it goes to.
    pub write: Option<Placement>,
    /// A received RoCE frame ended this cycle.
    pub done: bool,
    /// What became of it.
    pub outcome: RxOutcome,
    /// How many bytes it placed.
    pub placed: Bits<13>,
    /// Its headers, for observability.
    pub headers: Option<RxHeaders>,
    /// A connection-manager reply byte.
    pub reply: Option<Bits<8>>,
    /// This reply byte is the first of its frame.
    pub reply_sof: bool,
    /// This reply byte is the last of its frame.
    pub reply_eof: bool,
    /// A connection-manager request was decided this cycle.
    pub cm_done: bool,
    /// What the manager made of it.
    pub cm_status: CmStatus,
    /// A connection-manager write was dropped because the local
    /// processor presented one in the same cycle. See the module docs.
    pub cm_overridden: bool,
    /// A RoCE frame is in progress at the receiver.
    pub rx_busy: bool,
    /// The manager's phase, for waveform reading.
    pub cm_phase: CmPhase,
}

impl<const QPN_W: usize, const RKEY_W: usize> SynchronousIO for RoceUcRxNode<QPN_W, RKEY_W>
where
    rhdl::bits::W<QPN_W>: BitWidth,
    rhdl::bits::W<RKEY_W>: BitWidth,
{
    type I = In;
    type O = Out;
    type Kernel = roce_uc_rx_node<QPN_W, RKEY_W>;
}

#[kernel]
// `if let` is not in the kernel-accepted subset, so the single-arm
// matches are the only spelling available. Same reasoning and the same
// allow as `crate::rx::endpoint`.
#[allow(clippy::single_match, clippy::collapsible_match)]
/// Kernel for [`RoceUcRxNode`].
pub fn roce_uc_rx_node<const QPN_W: usize, const RKEY_W: usize>(
    cr: ClockReset,
    i: In,
    q: Q<QPN_W, RKEY_W>,
) -> (Out, D<QPN_W, RKEY_W>)
where
    rhdl::bits::W<QPN_W>: BitWidth,
    rhdl::bits::W<RKEY_W>: BitWidth,
{
    let mut d = D::<QPN_W, RKEY_W>::dont_care();
    let mut o = Out::dont_care();

    // The same bytes to both children.
    d.cm.data = i.data;
    d.cm.start_of_frame = i.start_of_frame;
    d.cm.end_of_frame = i.end_of_frame;
    d.cm.cfg = i.cfg;
    d.cm.port = i.cm_port;
    d.cm.ready = i.reply_ready;

    d.rx.data = i.data;
    d.rx.start_of_frame = i.start_of_frame;
    d.rx.end_of_frame = i.end_of_frame;

    // Configuration: the local processor outranks a remote peer.
    let mut qp = q.cm.qp_write;
    let mut mr = q.cm.mr_write;
    let mut overridden = false;
    match i.qp_write {
        Some(w) => {
            match q.cm.qp_write {
                Some(_c) => {
                    overridden = true;
                }
                None => {}
            }
            qp = Some(w);
        }
        None => {}
    }
    match i.mr_write {
        Some(w) => {
            match q.cm.mr_write {
                Some(_c) => {
                    overridden = true;
                }
                None => {}
            }
            mr = Some(w);
        }
        None => {}
    }
    d.rx.qp_write = qp;
    d.rx.mr_write = mr;

    o.write = q.rx.write;
    o.done = q.rx.done;
    o.outcome = q.rx.outcome;
    o.placed = q.rx.placed;
    o.headers = q.rx.headers;
    o.rx_busy = q.rx.busy;
    o.reply = q.cm.reply;
    o.reply_sof = q.cm.reply_sof;
    o.reply_eof = q.cm.reply_eof;
    o.cm_done = q.cm.done;
    o.cm_status = q.cm.status;
    o.cm_overridden = overridden;
    o.cm_phase = q.cm.phase;

    if cr.reset.any() {
        d.cm.data = None;
        d.cm.start_of_frame = false;
        d.cm.end_of_frame = false;
        d.cm.ready = false;
        d.rx.data = None;
        d.rx.start_of_frame = false;
        d.rx.end_of_frame = false;
        d.rx.qp_write = None;
        d.rx.mr_write = None;
        o.write = None;
        o.done = false;
        o.outcome = RxOutcome::Idle;
        o.placed = bits(0);
        o.headers = None;
        o.reply = None;
        o.reply_sof = false;
        o.reply_eof = false;
        o.cm_done = false;
        o.cm_status = CmStatus::Accepted;
        o.cm_overridden = false;
        o.rx_busy = false;
        o.cm_phase = CmPhase::Idle;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cm::message::CM_UDP_PORT;
    use crate::qp::context::{Pmtu, QpContext};
    use crate::qp::mr::MemoryRegion;
    use expect_test::expect;

    fn cfg() -> FrameConfig {
        FrameConfig {
            dst_mac: bits(0),
            src_mac: bits(0x000a_3501_0203),
            src_ip: bits(0xC0A8_010A),
            dst_ip: bits(0),
            src_port: bits(0),
            dst_port: bits(0),
            ip_id: bits(0),
            ttl: bits(64),
            dscp_ecn: bits(0),
        }
    }

    fn idle() -> In {
        In {
            data: None,
            start_of_frame: false,
            end_of_frame: false,
            cfg: cfg(),
            cm_port: bits(CM_UDP_PORT),
            reply_ready: true,
            qp_write: None,
            mr_write: None,
        }
    }

    /// **The local processor outranks a remote peer.** Pinned at the
    /// kernel level, because the condition is one line and the
    /// consequence of getting it backwards is that a peer can overwrite
    /// a configuration the host just installed.
    #[test]
    fn a_direct_write_wins_over_the_connection_manager() {
        let host_qp = QpWrite {
            qpn: bits(0x000999),
            ctx: Some(QpContext {
                dqpn: bits(0x000999),
                transport: crate::headers::Transport::Rc,
                pmtu: Pmtu::B256,
                rkey: bits(0x1111_1111),
                pkey: bits(0xFFFF),
            }),
        };
        let cm_qp = QpWrite {
            qpn: bits(0x000111),
            ctx: None,
        };
        let host_mr = MrWrite {
            rkey: bits(0x1111_1111),
            region: Some(MemoryRegion {
                rkey: bits(0x1111_1111),
                va_base: bits(0),
                length: bits(16),
                local_base: bits(0),
                remote_write: true,
            }),
        };
        let cm_mr = MrWrite {
            rkey: bits(0x0000_A5A5),
            region: None,
        };

        // Both present in the same cycle.
        let q = Q::<4, 4> {
            cm: crate::cm::manager::Out {
                qp_write: Some(cm_qp),
                mr_write: Some(cm_mr),
                reply: None,
                reply_sof: false,
                reply_eof: false,
                status: CmStatus::Accepted,
                done: true,
                busy: false,
                phase: CmPhase::Idle,
            },
            rx: crate::rx::endpoint::Out {
                write: None,
                done: false,
                outcome: RxOutcome::Idle,
                placed: bits(0),
                headers: None,
                busy: false,
            },
        };
        let i = In {
            qp_write: Some(host_qp),
            mr_write: Some(host_mr),
            ..idle()
        };
        let (o, d) = roce_uc_rx_node::<4, 4>(clock_reset(clock(false), reset(false)), i, q);
        assert_eq!(d.rx.qp_write, Some(host_qp), "the host's write is applied");
        assert_eq!(d.rx.mr_write, Some(host_mr));
        assert!(
            o.cm_overridden,
            "and the dropped manager write is reported, not silently lost"
        );

        // With no host write, the manager's goes through.
        let (o2, d2) = roce_uc_rx_node::<4, 4>(clock_reset(clock(false), reset(false)), idle(), q);
        assert_eq!(d2.rx.qp_write, Some(cm_qp));
        assert_eq!(d2.rx.mr_write, Some(cm_mr));
        assert!(!o2.cm_overridden, "nothing was overridden");
    }

    #[test]
    fn reset_quiesces_everything() {
        let q = Q::<4, 4>::dont_care();
        let i = In {
            data: Some(bits(0x42)),
            start_of_frame: true,
            ..idle()
        };
        let (o, d) = roce_uc_rx_node::<4, 4>(clock_reset(clock(false), reset(true)), i, q);
        assert_eq!(o.write, None);
        assert_eq!(o.reply, None);
        assert!(!o.done && !o.cm_done && !o.cm_overridden);
        assert_eq!(d.rx.qp_write, None, "no configuration under reset");
        assert_eq!(d.rx.mr_write, None);
        assert_eq!(d.cm.data, None);
        assert_eq!(d.rx.data, None);
    }

    // ------------------------------------------ Tiers 3, 4 and 5

    fn test_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In)>> {
        // Sixty arbitrary bytes: neither child will claim them, which
        // is the cheap case to round-trip through iverilog. The
        // protocol behaviour is covered by `tests/loopback.rs`.
        let mut stream = vec![idle()];
        let bytes: Vec<u8> = (0..60u8).collect();
        let last = bytes.len() - 1;
        for (k, b) in bytes.iter().enumerate() {
            stream.push(In {
                data: Some(bits(*b as u128)),
                start_of_frame: k == 0,
                end_of_frame: k == last,
                ..idle()
            });
        }
        stream.push(idle());
        stream.into_iter().with_reset(1).clock_pos_edge(100)
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut: RoceUcRxNode<4, 4> = RoceUcRxNode::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        assert!(hdl.contains("module top"));
        Ok(())
    }

    #[test]
    fn test_widget_hdl_works() -> miette::Result<()> {
        let uut: RoceUcRxNode<4, 4> = RoceUcRxNode::default();
        let tb = uut
            .run(test_stream())
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_widget_trace() -> miette::Result<()> {
        let uut: RoceUcRxNode<4, 4> = RoceUcRxNode::default();
        let vcd = uut.run(test_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("cm_node");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["7779224c9dea763bff44fdbe92450db972137063b8257850c91199aa8a88089f"];
        let digest = vcd.dump_to_file(root.join("cm_node.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
