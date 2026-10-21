//! The AXI4 channel beats, their wire encodings, and the burst
//! arithmetic.
//!
//! Nothing here is a widget. These are the types the five channels
//! carry and the pure functions that say where a burst's beats land —
//! which is the part of AXI4 most often got wrong, so it is written
//! down once, here, and tested on its own before anything drives it.
//!
//! # The channels
//!
//! Five independent ready/valid streams, exactly as the spec has them.
//! The handshake itself is [`crate::axi4lite::channel`], which is
//! already generic over `T: Digital` and is reused unchanged — the only
//! thing AXI4 adds to a channel is what rides on it.
//!
//! | Channel | Beat type | Direction |
//! |---|---|---|
//! | AW, AR | [`AxiAddress`] | manager → subordinate |
//! | W | [`AxiWriteBeat`] | manager → subordinate |
//! | B | [`AxiWriteResponse`] | subordinate → manager |
//! | R | [`AxiReadBeat`] | subordinate → manager |
//!
//! AW and AR carry the *same* beat type. The spec gives them identical
//! field sets, and giving them one type means the burst arithmetic is
//! written once rather than twice.
//!
//! # Why the strobe width is a second parameter
//!
//! [`AxiWriteBeat`] is `AxiWriteBeat<D, STRB>` where `STRB` must be
//! `D / 8`, rather than the obvious `Bits<{ D / 8 }>`. That spelling
//! does not compile on stable Rust:
//!
//! ```text
//! error: generic parameters may not be used in const operations
//! ```
//!
//! It needs `generic_const_exprs`, which is unstable. So the invariant
//! the type system cannot state is stated by
//! [`AxiWriteBeat::check_widths`] instead, which a caller can force at
//! compile time:
//!
//! ```ignore
//! const _: () = AxiWriteBeat::<32, 4>::check_widths();
//! ```

use rhdl::prelude::*;

/// The widest data bus this implementation supports, in bits.
///
/// Exactly the `Bits<N>` ceiling: the type exists for `N` in `1..=128`.
/// A wider bus needs an array-of-words payload or a kernel-language
/// change, and `axi4-architecture.md` §9 records that neither is
/// foreclosed.
pub const AXI4_MAX_DATA_BITS: usize = 128;

/// The largest `AxSIZE` a bus of [`AXI4_MAX_DATA_BITS`] can use:
/// `log2(128 / 8)` = 4, so sixteen bytes per beat.
pub const AXI4_MAX_SIZE: u128 = 4;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// `AxBURST` — how a burst's address advances.
///
/// Three variants, and the spec's fourth encoding (`0b11`) is
/// **absent** rather than present-and-reserved. An unrepresentable
/// illegal state is the same argument as
/// [`crate::rcstream`]'s typed framing and `rhdl-roce`'s
/// `WriteOpcode`: [`decode_burst`] returns `None` for it, so a
/// subordinate reports a named verdict instead of coercing it to
/// something plausible.
pub enum BurstKind {
    /// Every beat uses the same address. For peripheral FIFOs, where
    /// the address is a port rather than a location. Limited to 16
    /// beats.
    Fixed,
    #[default]
    /// Each beat advances by the transfer size. Up to 256 beats, and
    /// must not cross a 4 KiB boundary.
    Incr,
    /// Advances like `Incr` but wraps within an aligned block. Limited
    /// to 2, 4, 8 or 16 beats, and the start address must be
    /// size-aligned.
    Wrap,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// `RRESP` / `BRESP` — the outcome of a transfer.
///
/// All four encodings are legal, so unlike [`BurstKind`] there is no
/// `None` case to decode into.
pub enum AxiResponse {
    #[default]
    /// Normal access success.
    Okay,
    /// Exclusive access success. Only meaningful for a transfer that
    /// asked for one.
    ExOkay,
    /// The subordinate reached, and refused or failed.
    SlvErr,
    /// No subordinate at that address — the interconnect's decode
    /// failed.
    DecErr,
}

#[kernel]
/// The wire encoding of a burst kind.
pub fn encode_burst(b: BurstKind) -> Bits<2> {
    match b {
        BurstKind::Fixed => bits(0),
        BurstKind::Incr => bits(1),
        BurstKind::Wrap => bits(2),
    }
}

#[kernel]
/// Decode `AxBURST`, or `None` for the reserved encoding.
///
/// Returning `Option` rather than defaulting is the point. `0b11`
/// coerced to `Incr` would make a subordinate advance addresses for a
/// transfer whose intent it does not know, and write them somewhere.
pub fn decode_burst(v: Bits<2>) -> Option<BurstKind> {
    if v == bits(0) {
        Some(BurstKind::Fixed)
    } else if v == bits(1) {
        Some(BurstKind::Incr)
    } else if v == bits(2) {
        Some(BurstKind::Wrap)
    } else {
        None
    }
}

#[kernel]
/// The wire encoding of a response.
pub fn encode_response(r: AxiResponse) -> Bits<2> {
    match r {
        AxiResponse::Okay => bits(0),
        AxiResponse::ExOkay => bits(1),
        AxiResponse::SlvErr => bits(2),
        AxiResponse::DecErr => bits(3),
    }
}

#[kernel]
/// Decode `RRESP`/`BRESP`. Total, because all four encodings are legal.
pub fn decode_response(v: Bits<2>) -> AxiResponse {
    if v == bits(0) {
        AxiResponse::Okay
    } else if v == bits(1) {
        AxiResponse::ExOkay
    } else if v == bits(2) {
        AxiResponse::SlvErr
    } else {
        AxiResponse::DecErr
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// An address-phase beat — the payload of both AW and AR.
///
/// `A` is the address width and `ID` the transaction-ID width. `ID = 1`
/// is the narrowest useful setting; the spec permits an interconnect to
/// widen it as transactions pass through, which is why it is a
/// parameter rather than a constant.
pub struct AxiAddress<const A: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    /// `AxID`. Transfers with the same ID complete in order; different
    /// IDs may complete out of order.
    pub id: Bits<ID>,
    /// `AxADDR` — the address of the burst's *first* transfer. Later
    /// beats follow from [`beat_address`].
    pub addr: Bits<A>,
    /// `AxLEN`, encoded as beats **minus one**, so 0 means a
    /// single-beat burst and 255 means 256 beats.
    ///
    /// The off-by-one is the spec's, not a convenience here, and it is
    /// the reason [`burst_beats`] exists rather than callers adding one
    /// in a dozen places.
    pub len: Bits<8>,
    /// `AxSIZE` — `log2` of the bytes per beat. Must not exceed the bus
    /// width; see [`AXI4_MAX_SIZE`].
    pub size: Bits<3>,
    /// `AxBURST`.
    pub burst: BurstKind,
    /// `AxLOCK`. One bit in AXI4 (two in AXI3): an exclusive access.
    pub lock: bool,
    /// `AxCACHE`. Carried and forwarded; no logic behind it here.
    pub cache: Bits<4>,
    /// `AxPROT`. Carried and forwarded.
    pub prot: Bits<3>,
    /// `AxQOS`. Carried and forwarded.
    pub qos: Bits<4>,
    /// `AxREGION`. Carried and forwarded.
    pub region: Bits<4>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// A write-data beat — the payload of W.
///
/// `STRB` must equal `D / 8`; see the module docs on why that is a
/// parameter and [`AxiWriteBeat::check_widths`] for forcing it at
/// compile time.
pub struct AxiWriteBeat<const D: usize, const STRB: usize>
where
    rhdl::bits::W<D>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
{
    /// `WDATA`.
    pub data: Bits<D>,
    /// `WSTRB` — one bit per byte lane, set for the lanes this beat
    /// actually writes. Reduced on the first beat of an unaligned
    /// burst and on any narrow transfer.
    pub strb: Bits<STRB>,
    /// `WLAST` — asserted on the final beat and no other.
    pub last: bool,
}

impl<const D: usize, const STRB: usize> AxiWriteBeat<D, STRB>
where
    rhdl::bits::W<D>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
{
    /// The invariant the type system cannot state: `STRB == D / 8`.
    ///
    /// Force it at compile time with
    /// `const _: () = AxiWriteBeat::<32, 4>::check_widths();`.
    pub const fn check_widths() {
        assert!(
            STRB * 8 == D,
            "AxiWriteBeat<D, STRB> requires STRB == D / 8"
        );
        assert!(
            D <= AXI4_MAX_DATA_BITS,
            "data bus wider than AXI4_MAX_DATA_BITS"
        );
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// A read-data beat — the payload of R.
pub struct AxiReadBeat<const D: usize, const ID: usize>
where
    rhdl::bits::W<D>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    /// `RID` — which transaction this beat belongs to. Beats for
    /// different IDs may interleave; beats for one ID may not reorder.
    pub id: Bits<ID>,
    /// `RDATA`.
    pub data: Bits<D>,
    /// `RRESP` — **per beat**, unlike `BRESP`.
    pub resp: AxiResponse,
    /// `RLAST`.
    pub last: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// A write response — the payload of B. One per write transaction, not
/// per beat.
pub struct AxiWriteResponse<const ID: usize>
where
    rhdl::bits::W<ID>: BitWidth,
{
    /// `BID`.
    pub id: Bits<ID>,
    /// `BRESP`.
    pub resp: AxiResponse,
}

// ------------------------------------------------ burst arithmetic

#[kernel]
/// Beats in a burst: `AxLEN + 1`.
///
/// Returns 9 bits because 256 does not fit in 8.
pub fn burst_beats(len: Bits<8>) -> Bits<9> {
    len.resize::<9>() + bits(1)
}

#[kernel]
/// Bytes per beat: `1 << AxSIZE`.
///
/// 5 bits because `AxSIZE` is 3 bits, so the widest is `1 << 7` = 128
/// — larger than any bus here, but the field permits it and a
/// subordinate has to be able to compute it before deciding the
/// request is too wide.
pub fn bytes_per_beat(size: Bits<3>) -> Bits<9> {
    let one = bits::<9>(1);
    one << size
}

#[kernel]
/// Does this `AxSIZE` fit a bus of `bus_bytes` bytes?
///
/// A manager asking for more bytes per beat than the bus has is a
/// protocol violation, not something to clamp.
pub fn size_fits_bus(size: Bits<3>, bus_bytes: Bits<9>) -> bool {
    bytes_per_beat(size) <= bus_bytes
}

#[kernel]
/// Total bytes a burst moves: beats × bytes-per-beat.
pub fn burst_bytes(len: Bits<8>, size: Bits<3>) -> Bits<18> {
    let beats = burst_beats(len).resize::<18>();
    let per = bytes_per_beat(size).resize::<18>();
    beats * per
}

#[kernel]
/// Is this a legal `Wrap` burst length?
///
/// The spec allows only 2, 4, 8 and 16 beats, so `AxLEN` must be 1, 3,
/// 7 or 15.
pub fn wrap_len_is_legal(len: Bits<8>) -> bool {
    len == bits(1) || len == bits(3) || len == bits(7) || len == bits(15)
}

#[kernel]
/// The address of beat `beat` in a burst.
///
/// `Fixed` holds the start address. `Incr` advances by the transfer
/// size from the *aligned* start — the unaligned part of the first
/// address affects only its strobes, not where later beats land.
/// `Wrap` advances the same way but stays inside an aligned block of
/// `beats × size` bytes.
///
/// # On the modulo that is not a modulo
///
/// A `Wrap` burst's block length is `beats × size`, and both factors
/// are powers of two — 2, 4, 8 or 16 beats of `1 << AxSIZE` bytes. So
/// the wrap is a **mask**, not a division, which is what makes it
/// synthesizable without a divider. A reading of the spec that
/// implemented it with `%` would need
/// [`crate::core::divider`] and a multi-cycle stall for something that
/// is one AND gate wide.
pub fn beat_address<const A: usize>(
    start: Bits<A>,
    size: Bits<3>,
    len: Bits<8>,
    burst: BurstKind,
    beat: Bits<8>,
) -> Bits<A>
where
    rhdl::bits::W<A>: BitWidth,
{
    let bytes = bytes_per_beat(size).resize::<A>();
    // The aligned base: every beat after the first sits on a multiple
    // of the transfer size.
    let aligned = start & !(bytes - bits::<A>(1));
    let offset = beat.resize::<A>() * bytes;
    let incr = aligned + offset;

    // Wrap: keep `incr` inside the aligned block containing `start`.
    let block = burst_beats(len).resize::<A>() * bytes;
    let block_mask = block - bits::<A>(1);
    let block_lo = start & !block_mask;
    let wrapped = block_lo | (incr & block_mask);

    match burst {
        BurstKind::Fixed => start,
        BurstKind::Incr => incr,
        BurstKind::Wrap => wrapped,
    }
}

#[kernel]
/// Does an `Incr` burst cross a 4 KiB boundary?
///
/// A manager obligation, so a subordinate's job is to **detect and
/// report** it rather than to handle it. It is the most commonly
/// violated rule in AXI4 and it gets its own assertion in the
/// monitors.
///
/// `Fixed` cannot cross one — every beat is the same address. `Wrap`
/// cannot either: its block is aligned and no larger than 16 × 128
/// bytes, which is within 4 KiB.
pub fn crosses_4k<const A: usize>(
    start: Bits<A>,
    size: Bits<3>,
    len: Bits<8>,
    burst: BurstKind,
) -> bool
where
    rhdl::bits::W<A>: BitWidth,
{
    let bytes = bytes_per_beat(size).resize::<A>();
    let aligned = start & !(bytes - bits::<A>(1));
    let last_beat = burst_beats(len).resize::<A>() - bits::<A>(1);
    let last_addr = aligned + last_beat * bytes;
    let page = bits::<A>(0xFFF);
    // Comparing the *start of the last beat* against the start of the
    // burst is equivalent to comparing the last byte accessed, and it
    // is worth saying why: `last_addr` is size-aligned and a beat is
    // `bytes` long with `bytes <= 128`, so an aligned access of that
    // size can never straddle a 4 KiB boundary itself. The last byte is
    // therefore always in `last_addr`'s page.
    //
    // Same page iff the addresses agree above the low 12 bits.
    let crossed = (start & !page) != (last_addr & !page);
    match burst {
        BurstKind::Fixed => false,
        BurstKind::Incr => crossed,
        BurstKind::Wrap => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhdl::core::sim::testbench::kernel::test_kernel_vm_and_verilog_synchronous;

    const BURSTS: [BurstKind; 3] = [BurstKind::Fixed, BurstKind::Incr, BurstKind::Wrap];
    const RESPONSES: [AxiResponse; 4] = [
        AxiResponse::Okay,
        AxiResponse::ExOkay,
        AxiResponse::SlvErr,
        AxiResponse::DecErr,
    ];

    /// **The codecs are exact inverses, exhaustively over the whole
    /// 2-bit space.** No sampling, so nothing to miss.
    #[test]
    fn burst_encoding_is_exactly_invertible_and_reserved_is_refused() {
        let mut legal = std::collections::BTreeSet::new();
        for b in BURSTS {
            let v = encode_burst(b);
            legal.insert(v.raw());
            assert_eq!(decode_burst(v), Some(b), "{b:?} did not round-trip");
        }
        assert_eq!(legal, [0u128, 1, 2].into_iter().collect());
        for raw in 0u128..4 {
            let got = decode_burst(bits::<2>(raw));
            assert_eq!(
                got.is_some(),
                legal.contains(&raw),
                "{raw:#b} decoded to {got:?}"
            );
        }
        assert_eq!(
            decode_burst(bits(0b11)),
            None,
            "the reserved encoding must be refused, not coerced -- an \
             AxBURST read as Incr would advance addresses for a \
             transfer whose intent is unknown"
        );
    }

    #[test]
    fn response_encoding_is_exactly_invertible_and_total() {
        for r in RESPONSES {
            assert_eq!(decode_response(encode_response(r)), r, "{r:?}");
        }
        // Total: all four encodings map to something, and distinctly.
        // `AxiResponse` is not `Ord`, so compare via the encoding --
        // which is sound here because the round-trip above has already
        // established the encoding is injective.
        let seen: std::collections::BTreeSet<u128> = (0u128..4)
            .map(|v| encode_response(decode_response(bits::<2>(v))).raw())
            .collect();
        assert_eq!(seen.len(), 4, "all four responses must be distinct");
    }

    // ------------------------------------------- burst arithmetic

    /// The host reimplementation every arithmetic test compares
    /// against.
    ///
    /// Written from the spec independently of the kernel — if both were
    /// derived from the same expression the comparison would prove
    /// only that the expression is stable.
    fn host_beat_address(start: u128, size: u32, len: u32, burst: BurstKind, beat: u32) -> u128 {
        let bytes = 1u128 << size;
        let aligned = start & !(bytes - 1);
        let incr = aligned + (beat as u128) * bytes;
        match burst {
            BurstKind::Fixed => start,
            BurstKind::Incr => incr,
            BurstKind::Wrap => {
                let block = (len as u128 + 1) * bytes;
                let lo = start & !(block - 1);
                lo + (incr % block)
            }
        }
    }

    #[test]
    fn beat_counts_and_sizes() {
        assert_eq!(burst_beats(bits(0)).raw(), 1, "AxLEN is beats minus one");
        assert_eq!(burst_beats(bits(255)).raw(), 256, "and 256 needs 9 bits");
        for sz in 0u128..8 {
            assert_eq!(bytes_per_beat(bits::<3>(sz)).raw(), 1 << sz, "size {sz}");
        }
        assert_eq!(bytes_per_beat(bits(AXI4_MAX_SIZE)).raw(), 16);
        assert_eq!(burst_bytes(bits(15), bits(2)).raw(), 16 * 4);
    }

    #[test]
    fn a_size_wider_than_the_bus_is_refused() {
        // A 128-bit bus is 16 bytes.
        let bus = bits::<9>(16);
        for sz in 0u128..=AXI4_MAX_SIZE {
            assert!(size_fits_bus(bits::<3>(sz), bus), "size {sz} fits 16 bytes");
        }
        for sz in (AXI4_MAX_SIZE + 1)..8 {
            assert!(
                !size_fits_bus(bits::<3>(sz), bus),
                "size {sz} is {} bytes and must not fit a 16-byte bus",
                1 << sz
            );
        }
    }

    /// **The address of every beat of every burst kind**, swept against
    /// the host arithmetic.
    #[test]
    fn beat_addresses_match_the_arithmetic() {
        for burst in BURSTS {
            for size in 0u32..=4 {
                // Wrap is only legal at 2, 4, 8 or 16 beats; the others
                // are swept more widely.
                let lens: Vec<u32> = match burst {
                    BurstKind::Wrap => vec![1, 3, 7, 15],
                    BurstKind::Fixed => vec![0, 1, 7, 15],
                    BurstKind::Incr => vec![0, 1, 7, 15, 63, 255],
                };
                for len in lens {
                    for start in [0u128, 4, 0x40, 0x1000, 0x1234, 0xF000] {
                        for beat in [0u32, 1, len / 2, len] {
                            let got = beat_address::<32>(
                                bits(start),
                                bits(size as u128),
                                bits(len as u128),
                                burst,
                                bits(beat as u128),
                            );
                            let want =
                                host_beat_address(start, size, len, burst, beat) & 0xFFFF_FFFF;
                            assert_eq!(
                                got.raw(),
                                want,
                                "{burst:?} size={size} len={len} start={start:#x} \
                                 beat={beat}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// **A `Wrap` burst stays inside its block**, which is the property
    /// the mask is there to provide and the one a `%`-based reading
    /// would get right only by accident.
    #[test]
    fn a_wrap_burst_never_leaves_its_aligned_block() {
        for size in 0u32..=4 {
            for len in [1u32, 3, 7, 15] {
                let bytes = 1u128 << size;
                let block = (len as u128 + 1) * bytes;
                // Start must be size-aligned for a legal Wrap.
                for start in [0u128, bytes, block, block + bytes, 0x1000 + bytes] {
                    let lo = start & !(block - 1);
                    for beat in 0..=len {
                        let a = beat_address::<32>(
                            bits(start),
                            bits(size as u128),
                            bits(len as u128),
                            BurstKind::Wrap,
                            bits(beat as u128),
                        )
                        .raw();
                        assert!(
                            a >= lo && a < lo + block,
                            "size={size} len={len} start={start:#x} beat={beat}: \
                             {a:#x} escaped block [{lo:#x}, {:#x})",
                            lo + block
                        );
                    }
                }
            }
        }
    }

    /// Every beat of a `Wrap` burst is distinct — it visits each
    /// address in the block exactly once.
    #[test]
    fn a_wrap_burst_visits_each_address_once() {
        for size in 0u32..=4 {
            for len in [1u32, 3, 7, 15] {
                let bytes = 1u128 << size;
                let start = 3 * bytes & !(bytes - 1);
                let seen: std::collections::BTreeSet<u128> = (0..=len)
                    .map(|beat| {
                        beat_address::<32>(
                            bits(start),
                            bits(size as u128),
                            bits(len as u128),
                            BurstKind::Wrap,
                            bits(beat as u128),
                        )
                        .raw()
                    })
                    .collect();
                assert_eq!(
                    seen.len(),
                    (len + 1) as usize,
                    "size={size} len={len}: a wrap burst must not revisit an \
                     address"
                );
            }
        }
    }

    #[test]
    fn only_four_wrap_lengths_are_legal() {
        for len in 0u128..256 {
            let ok = wrap_len_is_legal(bits::<8>(len));
            let want = matches!(len, 1 | 3 | 7 | 15);
            assert_eq!(ok, want, "AxLEN {len} ({} beats)", len + 1);
        }
    }

    /// **The 4 KiB rule**, which is the most commonly violated in AXI4.
    #[test]
    fn the_four_kilobyte_boundary_is_detected() {
        // 2 beats of 4 bytes from 0xFFC: touches 0xFFC..0x1003.
        assert!(
            crosses_4k::<32>(bits(0xFFC), bits(2), bits(1), BurstKind::Incr),
            "0xFFC + 2 beats of 4 crosses into the next page"
        );
        // The same burst one beat shorter stays put.
        assert!(
            !crosses_4k::<32>(bits(0xFFC), bits(2), bits(0), BurstKind::Incr),
            "a single beat at 0xFFC ends at 0xFFF, still in page 0"
        );
        // A full 256-beat burst of 16 bytes is 4096 bytes: legal from an
        // aligned start, illegal from anywhere else.
        assert!(
            !crosses_4k::<32>(bits(0x2000), bits(4), bits(255), BurstKind::Incr),
            "4096 bytes from a 4 KiB-aligned start exactly fills a page"
        );
        assert!(
            crosses_4k::<32>(bits(0x2010), bits(4), bits(255), BurstKind::Incr),
            "the same burst 16 bytes further on must cross"
        );
        // Fixed and Wrap cannot cross, by construction.
        for b in [BurstKind::Fixed, BurstKind::Wrap] {
            assert!(
                !crosses_4k::<32>(bits(0xFFC), bits(2), bits(1), b),
                "{b:?} cannot cross a page boundary"
            );
        }
    }

    /// The boundary detection agrees with the beat addresses it is
    /// supposed to summarise — swept, so the two cannot drift.
    #[test]
    fn boundary_detection_agrees_with_the_beat_addresses() {
        for size in 0u32..=4 {
            for len in [0u32, 1, 7, 15, 255] {
                for start in (0u128..0x2000).step_by(0x1F) {
                    let reported = crosses_4k::<32>(
                        bits(start),
                        bits(size as u128),
                        bits(len as u128),
                        BurstKind::Incr,
                    );
                    let last =
                        host_beat_address(start, size, len, BurstKind::Incr, len) & 0xFFFF_FFFF;
                    let actual = (start & !0xFFF) != (last & !0xFFF);
                    assert_eq!(
                        reported, actual,
                        "size={size} len={len} start={start:#x}: reported \
                         {reported}, beats run {start:#x}..{last:#x}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_strobe_invariant_is_checkable_at_compile_time() {
        const _: () = AxiWriteBeat::<32, 4>::check_widths();
        const _: () = AxiWriteBeat::<128, 16>::check_widths();
        const _: () = AxiWriteBeat::<8, 1>::check_widths();
        let b = AxiWriteBeat::<128, 16> {
            data: bits(0),
            strb: bits(0xFFFF),
            last: true,
        };
        assert_eq!(b.strb.raw(), 0xFFFF);
    }

    // --------------------------------- VM and Verilog cross-checks

    #[test]
    fn codec_kernels_vm_and_verilog() -> miette::Result<()> {
        test_kernel_vm_and_verilog_synchronous::<decode_burst, _, _, _>(
            decode_burst,
            (0u128..4).map(|v| (bits::<2>(v),)),
        )?;
        test_kernel_vm_and_verilog_synchronous::<decode_response, _, _, _>(
            decode_response,
            (0u128..4).map(|v| (bits::<2>(v),)),
        )?;
        test_kernel_vm_and_verilog_synchronous::<encode_burst, _, _, _>(
            encode_burst,
            BURSTS.into_iter().map(|b| (b,)),
        )?;
        test_kernel_vm_and_verilog_synchronous::<encode_response, _, _, _>(
            encode_response,
            RESPONSES.into_iter().map(|r| (r,)),
        )?;
        Ok(())
    }

    #[test]
    fn arithmetic_kernels_vm_and_verilog() -> miette::Result<()> {
        test_kernel_vm_and_verilog_synchronous::<burst_beats, _, _, _>(
            burst_beats,
            (0u128..256).map(|n| (bits::<8>(n),)),
        )?;
        test_kernel_vm_and_verilog_synchronous::<bytes_per_beat, _, _, _>(
            bytes_per_beat,
            (0u128..8).map(|n| (bits::<3>(n),)),
        )?;
        test_kernel_vm_and_verilog_synchronous::<wrap_len_is_legal, _, _, _>(
            wrap_len_is_legal,
            (0u128..256).map(|n| (bits::<8>(n),)),
        )?;
        Ok(())
    }

    #[test]
    fn beat_address_kernel_vm_and_verilog() -> miette::Result<()> {
        let mut inputs = Vec::new();
        for burst in BURSTS {
            for size in 0u128..=4 {
                for len in [1u128, 3, 7, 15] {
                    for start in [0u128, 0x40, 0x1234] {
                        for beat in [0u128, 1, len] {
                            inputs.push((
                                bits::<32>(start),
                                bits::<3>(size),
                                bits::<8>(len),
                                burst,
                                bits::<8>(beat),
                            ));
                        }
                    }
                }
            }
        }
        test_kernel_vm_and_verilog_synchronous::<beat_address<32>, _, _, _>(
            beat_address::<32>,
            inputs.into_iter(),
        )?;
        Ok(())
    }

    #[test]
    fn crosses_4k_kernel_vm_and_verilog() -> miette::Result<()> {
        let mut inputs = Vec::new();
        for burst in BURSTS {
            for size in 0u128..=4 {
                for len in [0u128, 15, 255] {
                    for start in [0u128, 0xFFC, 0x1000, 0x2010] {
                        inputs.push((bits::<32>(start), bits::<3>(size), bits::<8>(len), burst));
                    }
                }
            }
        }
        test_kernel_vm_and_verilog_synchronous::<crosses_4k<32>, _, _, _>(
            crosses_4k::<32>,
            inputs.into_iter(),
        )?;
        Ok(())
    }
}
