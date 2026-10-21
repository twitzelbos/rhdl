//! The connection-manager datagram: what a host sends to set a queue
//! pair up, and what comes back.
//!
//! # This is our protocol, not a standard one
//!
//! Stated plainly because it would be easy to assume otherwise. RDMA
//! connection establishment in the wider world is InfiniBand CM or
//! RDMA CM, which are large protocols carrying path records, service
//! IDs and private data. **This is not either of them.**
//! `roce-v2-architecture.md` §9 Phase 2 records the decision to do
//! "out-of-band connection setup over UDP, as FERoCE does", and this is
//! that: a fixed-layout 40-byte datagram, defined here, normative here,
//! and understood only by a peer that was written against this file.
//!
//! The consequence is the honest one: a stock Linux host will not
//! negotiate with this out of the box. A small userspace helper has to
//! send these datagrams, and that helper is the thing that reads
//! `ibv_reg_mr`'s results and passes them on. That is a deliberate
//! trade — implementing RDMA CM in hardware is a protocol stack in its
//! own right, and it is not what makes a sensor stream data.
//!
//! # Layout
//!
//! All fields big-endian, like every other field in this crate.
//! Reserved bytes are sent as zero and ignored on receipt, which is
//! what makes the layout extensible without a version bump.
//!
//! | Offset | Size | Field |
//! |---:|---:|---|
//! | 0 | 4 | magic, [`CM_MAGIC`] — `"RCM1"` |
//! | 4 | 1 | opcode, [`CmOpcode`] |
//! | 5 | 1 | status, [`CmStatus`] — zero in a request |
//! | 6 | 1 | transport service |
//! | 7 | 1 | path MTU code |
//! | 8 | 3 | queue-pair number to configure |
//! | 11 | 1 | reserved |
//! | 12 | 3 | the peer's queue-pair number |
//! | 15 | 1 | reserved |
//! | 16 | 3 | initial packet sequence number |
//! | 19 | 1 | reserved |
//! | 20 | 4 | R_Key for the region being registered |
//! | 24 | 8 | the region's base virtual address |
//! | 32 | 4 | the region's length |
//! | 36 | 4 | the local address the region maps to |
//!
//! Every multi-byte field is aligned to its own size, which is why the
//! three 24-bit fields each sit in a 4-byte slot with a reserved byte
//! after them rather than being packed tight. A packed layout would
//! save four bytes of a datagram sent once per connection, and cost a
//! reader having to count.
//!
//! # Why `local_base` is on the wire
//!
//! It is the one field a sceptical reader should stop at: the datagram
//! lets the peer name the *local* address its region maps to, which
//! sounds like handing out the keys.
//!
//! It is sound here because the connection manager is on the trusted
//! side of the link — it is how this device's own host configures it,
//! the same role a PCIe register write plays on a commodity RNIC. The
//! field exists so that configuration is one datagram instead of a
//! datagram plus a side-channel.
//!
//! **It is also the reason the connection-manager port must not be
//! reachable from an untrusted network.** That is a deployment
//! constraint, it is not enforced by anything in this crate, and it is
//! recorded here and in [`super::manager`] because a future change that
//! exposes this port is a privilege escalation and should be read as
//! one.

use crate::headers::Transport;
use crate::qp::context::Pmtu;
use rhdl::prelude::*;

/// `"RCM1"` — RHDL Connection Manager, version 1.
///
/// A magic number rather than relying on the port alone, because UDP
/// ports are not a namespace anyone owns and a stray datagram on this
/// port must not be able to reconfigure a queue pair.
pub const CM_MAGIC: u128 = 0x5243_4D31;

/// The datagram's length in bytes.
pub const CM_MSG_BYTES: u128 = 40;

/// The default UDP port for connection management.
///
/// One above the IANA-assigned RoCEv2 port, which is a convention of
/// this crate and nothing more. It is a *default*: the widget takes the
/// port as an input so a deployment can move it.
pub const CM_UDP_PORT: u128 = 4792;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// What a connection-manager datagram asks for.
pub enum CmOpcode {
    #[default]
    /// Configure a queue pair and register a memory region.
    Connect,
    /// Tear both down.
    Disconnect,
    /// A reply. Sent by this device, never acted on by it — a datagram
    /// arriving with this opcode is rejected, so two devices cannot
    /// talk each other into a loop of replies.
    Response,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// The outcome reported in a reply.
pub enum CmStatus {
    #[default]
    /// The request was applied.
    Accepted,
    /// The magic number was wrong.
    BadMagic,
    /// The opcode was not one this device acts on.
    BadOpcode,
    /// The transport-service or MTU code was not one this device
    /// implements.
    BadParameter,
    /// The region's length was zero, which cannot be written to.
    BadRegion,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// A decoded connection-manager datagram.
pub struct CmMessage {
    /// What is being asked.
    pub opcode: CmOpcode,
    /// The outcome, in a reply. Zero in a request.
    pub status: CmStatus,
    /// The transport service the connection will use.
    pub transport: Transport,
    /// The negotiated path MTU.
    pub pmtu: Pmtu,
    /// The queue pair to configure on this device.
    pub qpn: Bits<24>,
    /// The peer's queue-pair number — what this device will put in the
    /// BTH when it sends.
    pub remote_qpn: Bits<24>,
    /// The sequence number the connection starts at.
    pub psn: Bits<24>,
    /// The R_Key the peer will present when writing to us.
    pub rkey: Bits<32>,
    /// The region's base virtual address, in the peer's terms.
    pub va: Bits<64>,
    /// The region's length in bytes.
    pub length: Bits<32>,
    /// The local address the region's first byte maps to. See the
    /// module docs on why this is on the wire.
    pub local_base: Bits<32>,
}

#[kernel]
/// Byte `k` of a 32-bit value, `k = 0` being the most significant.
///
/// `k` is a [`Bits<2>`] so the shift is `(3 - k) * 8`, at most 24, and
/// cannot exceed the value's width. That is the point of the narrow
/// type: a kernel evaluates both arms of an `if`, so a shift whose
/// bound depends on a runtime comparison would trip the VM's check even
/// when the result is discarded.
pub fn byte_of32(v: Bits<32>, k: Bits<2>) -> Bits<8> {
    let sh = (bits::<5>(3) - k.resize::<5>()).resize::<32>() << 3;
    ((v >> sh) & bits(0xFF)).resize::<8>()
}

#[kernel]
/// Byte `k` of a 64-bit value, `k = 0` being the most significant.
///
/// `k` is a [`Bits<3>`], so the shift is at most 56. Same reasoning as
/// [`byte_of32`].
pub fn byte_of64(v: Bits<64>, k: Bits<3>) -> Bits<8> {
    let sh = (bits::<6>(7) - k.resize::<6>()).resize::<32>() << 3;
    ((v >> sh) & bits(0xFF)).resize::<8>()
}

#[kernel]
/// `index` clamped into `[lo, hi]` and rebased to zero.
///
/// The clamp is what makes a field's byte extraction safe to evaluate
/// at an index outside that field — which a kernel does, because every
/// arm of an `if`/`else` chain runs.
pub fn rel_index(index: Bits<6>, lo: Bits<6>, hi: Bits<6>) -> Bits<6> {
    let c = if index < lo {
        lo
    } else if index > hi {
        hi
    } else {
        index
    };
    c - lo
}

#[kernel]
/// The wire code for a transport service.
pub fn cm_transport_code(t: Transport) -> Bits<8> {
    match t {
        Transport::Rc => bits(0),
        Transport::Uc => bits(1),
    }
}

#[kernel]
/// Decode a transport-service code, or `None` if it is not one we
/// implement.
pub fn decode_cm_transport(b: Bits<8>) -> Option<Transport> {
    if b == bits(0) {
        Some(Transport::Rc)
    } else if b == bits(1) {
        Some(Transport::Uc)
    } else {
        None
    }
}

#[kernel]
/// The wire code for a path MTU.
///
/// The IBTA's own MTU enumeration — 1 through 5 for 256 up to 4096 —
/// rather than a private numbering, so the field means the same thing
/// it means everywhere else in RDMA.
pub fn cm_pmtu_code(p: Pmtu) -> Bits<8> {
    match p {
        Pmtu::B256 => bits(1),
        Pmtu::B512 => bits(2),
        Pmtu::B1024 => bits(3),
        Pmtu::B2048 => bits(4),
        Pmtu::B4096 => bits(5),
    }
}

#[kernel]
/// Decode a path-MTU code, or `None` if it is not one of the five the
/// IBTA defines.
pub fn decode_cm_pmtu(b: Bits<8>) -> Option<Pmtu> {
    if b == bits(1) {
        Some(Pmtu::B256)
    } else if b == bits(2) {
        Some(Pmtu::B512)
    } else if b == bits(3) {
        Some(Pmtu::B1024)
    } else if b == bits(4) {
        Some(Pmtu::B2048)
    } else if b == bits(5) {
        Some(Pmtu::B4096)
    } else {
        None
    }
}

#[kernel]
/// The wire code for an opcode.
pub fn cm_opcode_code(o: CmOpcode) -> Bits<8> {
    match o {
        CmOpcode::Connect => bits(1),
        CmOpcode::Disconnect => bits(2),
        CmOpcode::Response => bits(3),
    }
}

#[kernel]
/// Decode an opcode, or `None` if it is not one this device acts on.
///
/// [`CmOpcode::Response`] decodes successfully — a reply is a
/// well-formed datagram — and it is [`super::manager`] that refuses to
/// *act* on one. Keeping the distinction here means the codec stays a
/// codec.
pub fn decode_cm_opcode(b: Bits<8>) -> Option<CmOpcode> {
    if b == bits(1) {
        Some(CmOpcode::Connect)
    } else if b == bits(2) {
        Some(CmOpcode::Disconnect)
    } else if b == bits(3) {
        Some(CmOpcode::Response)
    } else {
        None
    }
}

#[kernel]
/// The wire code for a status.
pub fn cm_status_code(s: CmStatus) -> Bits<8> {
    match s {
        CmStatus::Accepted => bits(0),
        CmStatus::BadMagic => bits(1),
        CmStatus::BadOpcode => bits(2),
        CmStatus::BadParameter => bits(3),
        CmStatus::BadRegion => bits(4),
    }
}

#[kernel]
/// Byte `index` of a connection-manager datagram, `0 ..= 39`.
///
/// Every field's byte extraction clamps its index into that field's own
/// range before computing a shift, because a kernel's `if`/`else`
/// evaluates every arm — so the extraction for the `va` field runs even
/// when `index` is 4, and an unclamped `31 - index` would wrap to an
/// enormous shift and trip the VM's bound check.
pub fn cm_byte(m: CmMessage, index: Bits<6>) -> Bits<8> {
    let mut out = bits::<8>(0);
    if index < bits(4) {
        out = byte_of32(
            bits::<32>(CM_MAGIC),
            rel_index(index, bits(0), bits(3)).resize::<2>(),
        );
    } else if index == bits(4) {
        out = cm_opcode_code(m.opcode);
    } else if index == bits(5) {
        out = cm_status_code(m.status);
    } else if index == bits(6) {
        out = cm_transport_code(m.transport);
    } else if index == bits(7) {
        out = cm_pmtu_code(m.pmtu);
    } else if index < bits(11) {
        // 8..10: the low three bytes of a 24-bit field, so byte 1 of
        // its 32-bit widening.
        out = byte_of32(
            m.qpn.resize::<32>(),
            (rel_index(index, bits(8), bits(10)) + bits(1)).resize::<2>(),
        );
    } else if index < bits(12) {
        out = bits(0); // reserved
    } else if index < bits(15) {
        out = byte_of32(
            m.remote_qpn.resize::<32>(),
            (rel_index(index, bits(12), bits(14)) + bits(1)).resize::<2>(),
        );
    } else if index < bits(16) {
        out = bits(0); // reserved
    } else if index < bits(19) {
        out = byte_of32(
            m.psn.resize::<32>(),
            (rel_index(index, bits(16), bits(18)) + bits(1)).resize::<2>(),
        );
    } else if index < bits(20) {
        out = bits(0); // reserved
    } else if index < bits(24) {
        out = byte_of32(m.rkey, rel_index(index, bits(20), bits(23)).resize::<2>());
    } else if index < bits(32) {
        out = byte_of64(m.va, rel_index(index, bits(24), bits(31)).resize::<3>());
    } else if index < bits(36) {
        out = byte_of32(m.length, rel_index(index, bits(32), bits(35)).resize::<2>());
    } else if index < bits(40) {
        out = byte_of32(
            m.local_base,
            rel_index(index, bits(36), bits(39)).resize::<2>(),
        );
    }
    // No final `else`: `out` is initialised to zero above and an index
    // past the datagram's end leaves it there. Writing `else { out =
    // bits(0) }` would make the initialiser dead, which rustc warns
    // about -- and the warning would be right.
    out
}

#[kernel]
/// [`rel_index`] over a 7-bit index, for the reply frame's 82 offsets.
pub fn rel_index7(index: Bits<7>, lo: Bits<7>, hi: Bits<7>) -> Bits<7> {
    let c = if index < lo {
        lo
    } else if index > hi {
        hi
    } else {
        index
    };
    c - lo
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;

    fn sample() -> CmMessage {
        CmMessage {
            opcode: CmOpcode::Connect,
            status: CmStatus::Accepted,
            transport: Transport::Uc,
            pmtu: Pmtu::B1024,
            qpn: bits(0x01_2345),
            remote_qpn: bits(0xAB_CDEF),
            psn: bits(0x00_0001),
            rkey: bits(0xDEAD_BEEF),
            va: bits(0x0123_4567_89AB_CDEF),
            length: bits(4096),
            local_base: bits(0x0002_0000),
        }
    }

    #[test]
    fn byte_extraction_is_most_significant_first() {
        let v = bits::<32>(0x1122_3344);
        assert_eq!(byte_of32(v, bits(0)).raw(), 0x11);
        assert_eq!(byte_of32(v, bits(1)).raw(), 0x22);
        assert_eq!(byte_of32(v, bits(2)).raw(), 0x33);
        assert_eq!(byte_of32(v, bits(3)).raw(), 0x44);
        let w = bits::<64>(0x0102_0304_0506_0708);
        for k in 0..8u128 {
            assert_eq!(byte_of64(w, bits(k)).raw(), k + 1, "byte {k}");
        }
    }

    #[test]
    fn rel_index_clamps_both_ends() {
        assert_eq!(rel_index(bits(0), bits(8), bits(10)).raw(), 0, "below");
        assert_eq!(rel_index(bits(8), bits(8), bits(10)).raw(), 0);
        assert_eq!(rel_index(bits(9), bits(8), bits(10)).raw(), 1);
        assert_eq!(rel_index(bits(10), bits(8), bits(10)).raw(), 2);
        assert_eq!(rel_index(bits(63), bits(8), bits(10)).raw(), 2, "above");
        assert_eq!(rel_index7(bits(0), bits(14), bits(33)).raw(), 0);
        assert_eq!(rel_index7(bits(127), bits(14), bits(33)).raw(), 19);
    }

    /// Every coded field round-trips, and nothing outside the defined
    /// codes decodes. Exhaustive over all 256 byte values, so there is
    /// no sampling.
    #[test]
    fn every_code_round_trips_and_nothing_else_decodes() {
        let mut ok_t = std::collections::BTreeSet::new();
        for t in [Transport::Rc, Transport::Uc] {
            let c = cm_transport_code(t);
            ok_t.insert(c.raw());
            assert_eq!(decode_cm_transport(c), Some(t), "{t:?}");
        }
        let mut ok_p = std::collections::BTreeSet::new();
        for p in [
            Pmtu::B256,
            Pmtu::B512,
            Pmtu::B1024,
            Pmtu::B2048,
            Pmtu::B4096,
        ] {
            let c = cm_pmtu_code(p);
            ok_p.insert(c.raw());
            assert_eq!(decode_cm_pmtu(c), Some(p), "{p:?}");
        }
        let mut ok_o = std::collections::BTreeSet::new();
        for o in [CmOpcode::Connect, CmOpcode::Disconnect, CmOpcode::Response] {
            let c = cm_opcode_code(o);
            ok_o.insert(c.raw());
            assert_eq!(decode_cm_opcode(c), Some(o), "{o:?}");
        }
        // The IBTA's MTU numbering, stated rather than implied.
        assert_eq!(ok_p, [1u128, 2, 3, 4, 5].into_iter().collect());

        for raw in 0u128..256 {
            let b = bits::<8>(raw);
            assert_eq!(
                decode_cm_transport(b).is_some(),
                ok_t.contains(&raw),
                "transport {raw:#04x}"
            );
            assert_eq!(
                decode_cm_pmtu(b).is_some(),
                ok_p.contains(&raw),
                "MTU {raw:#04x}"
            );
            assert_eq!(
                decode_cm_opcode(b).is_some(),
                ok_o.contains(&raw),
                "opcode {raw:#04x}"
            );
        }
    }

    /// Status codes are distinct, so a reply cannot confuse two
    /// failures.
    #[test]
    fn status_codes_are_distinct() {
        let all = [
            CmStatus::Accepted,
            CmStatus::BadMagic,
            CmStatus::BadOpcode,
            CmStatus::BadParameter,
            CmStatus::BadRegion,
        ];
        let codes: std::collections::BTreeSet<u128> =
            all.iter().map(|s| cm_status_code(*s).raw()).collect();
        assert_eq!(codes.len(), all.len(), "distinct codes: {codes:?}");
        assert_eq!(cm_status_code(CmStatus::Accepted).raw(), 0, "success is 0");
    }

    /// **The datagram's bytes, checked against the layout table in the
    /// module docs by reading them back independently.**
    #[test]
    fn the_datagram_matches_its_documented_layout() {
        let m = sample();
        let f: Vec<u8> = (0..CM_MSG_BYTES)
            .map(|k| cm_byte(m, bits(k)).raw() as u8)
            .collect();
        assert_eq!(f.len(), 40);

        assert_eq!(&f[0..4], b"RCM1", "magic");
        assert_eq!(f[4], 1, "Connect");
        assert_eq!(f[5], 0, "Accepted");
        assert_eq!(f[6], 1, "UC");
        assert_eq!(f[7], 3, "MTU 1024");
        assert_eq!(&f[8..11], &[0x01, 0x23, 0x45], "qpn, 24 bits");
        assert_eq!(f[11], 0, "reserved");
        assert_eq!(&f[12..15], &[0xAB, 0xCD, 0xEF], "remote qpn");
        assert_eq!(f[15], 0, "reserved");
        assert_eq!(&f[16..19], &[0x00, 0x00, 0x01], "psn");
        assert_eq!(f[19], 0, "reserved");
        assert_eq!(&f[20..24], &[0xDE, 0xAD, 0xBE, 0xEF], "rkey");
        assert_eq!(
            &f[24..32],
            &[0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF],
            "va"
        );
        assert_eq!(&f[32..36], &4096u32.to_be_bytes(), "length");
        assert_eq!(&f[36..40], &0x0002_0000u32.to_be_bytes(), "local_base");
    }

    /// An index past the datagram reads zero rather than wrapping into
    /// another field -- which matters because a kernel evaluates every
    /// arm of the chain in `cm_byte`.
    #[test]
    fn an_index_past_the_end_is_zero() {
        let m = sample();
        for k in CM_MSG_BYTES..64 {
            assert_eq!(cm_byte(m, bits(k)).raw(), 0, "index {k}");
        }
    }

    #[test]
    fn cm_byte_kernel_vm_and_verilog() -> miette::Result<()> {
        let m = sample();
        let inputs = (0u128..64).map(move |k| (m, bits::<6>(k)));
        test_kernel_vm_and_verilog_synchronous::<cm_byte, _, _, _>(cm_byte, inputs)?;
        Ok(())
    }

    #[test]
    fn codec_kernels_vm_and_verilog() -> miette::Result<()> {
        test_kernel_vm_and_verilog_synchronous::<decode_cm_transport, _, _, _>(
            decode_cm_transport,
            (0u128..256).map(|n| (bits::<8>(n),)),
        )?;
        test_kernel_vm_and_verilog_synchronous::<decode_cm_pmtu, _, _, _>(
            decode_cm_pmtu,
            (0u128..256).map(|n| (bits::<8>(n),)),
        )?;
        test_kernel_vm_and_verilog_synchronous::<decode_cm_opcode, _, _, _>(
            decode_cm_opcode,
            (0u128..256).map(|n| (bits::<8>(n),)),
        )?;
        test_kernel_vm_and_verilog_synchronous::<byte_of32, _, _, _>(
            byte_of32,
            (0u128..4).map(|k| (bits::<32>(0x1122_3344), bits::<2>(k))),
        )?;
        test_kernel_vm_and_verilog_synchronous::<byte_of64, _, _, _>(
            byte_of64,
            (0u128..8).map(|k| (bits::<64>(0x0102_0304_0506_0708), bits::<3>(k))),
        )?;
        Ok(())
    }
}
