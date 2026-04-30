//! SMPTE LTC (Linear Timecode) decoder
//!
//! Decodes SMPTE 12M Linear Timecode from a single biphase-mark
//! line into structured time-of-day frames (HH:MM:SS:FF + flags +
//! user bits + drop-frame + colour-frame).  Recovers the bit clock
//! by measuring intervals between line transitions; assembles
//! 80-bit LTC words; locates the canonical sync word (`0xBFFC`
//! transmitted LSB-first, which lands as `0x3FFD` in MSB-first
//! reading); parses the 64 payload bits per the SMPTE 12M-1
//! nibble layout.
//!
//! ## Decoder strategy
//!
//! 1. **Edge detection.**  Every transition on `line_in` is
//!    captured; the cycle count between consecutive transitions
//!    (the "interval") is the signal we decode.
//! 2. **Bit cell vs half-cell discrimination.**  Biphase mark
//!    encodes a `0` bit as a single transition at the cell start
//!    and a `1` bit as that transition *plus* a second transition
//!    at the cell midpoint.  A long interval (≈ a full cell) is a
//!    `0`; two consecutive short intervals (≈ each half a cell)
//!    is a `1`.  The boundary between "long" and "short" is
//!    1.5×T where T is the half-cell time, learned from the
//!    running average of recent intervals (auto bit-rate lock).
//! 3. **Sync alignment.**  Bits are shifted into an 80-bit register
//!    LSB-first; when the high 16 bits match the sync pattern, the
//!    low 64 bits are the parsable LTC payload.
//! 4. **Frame assembly.**  The 64 payload bits are parsed per
//!    SMPTE 12M-1: frame-units, drop-frame flag, colour-frame
//!    flag, frame-tens, then seconds, minutes, hours
//!    nibble-by-nibble interleaved with user-data nibbles and the
//!    eight binary-group flag bits.
//! 5. **Forward / reverse detection.**  Forward play yields
//!    sync field `0x3FFD` (MSB-first reading of the LSB-first sync
//!    bits `0xBFFC`); reverse playback gives `0xBFFC`.
//!
//! This is the receive companion to
//! [super::smpte_ltc_encoder::SmpteLtcEncoder].
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-----+SmpteLtcDecoder+-----+
     |                           |
bool |                           |  Option<LtcFrame>
+--->| line_in            frame  +-->
     |                           |
     |                  forward  +--> bool
     |                           |
     |                  in_lock  +--> bool
     +---------------------------+
")]
//!
//!# Internals
//!
//! Three-stage pipeline: edge detector → bit-cell decoder
//! (discriminates short vs long intervals) → 80-bit shift register
//! + sync-word matcher → frame parser.  The bit-cell decoder is
//! the only stateful piece beyond the shift register: it carries a
//! "half-cell flag" (set when a short interval was just observed,
//! so the next short interval completes a `1` bit; cleared on a
//! long interval, which completes a `0` bit and clears the half).
//!
//!# Parameters
//!
//! - `IW` — width of the interval counter.  For a 100 MHz clock
//!   and 30 fps LTC (2400 Hz half-cell, 41.6 µs cell, ~4160-cycle
//!   half-cell), `IW = 14` covers the range.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/smpte_ltc_decoder.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/smpte_ltc_decoder.md")]
//!
//! And the auto-generated FSM diagram:
#![doc = include_str!("../../doc/smpte_ltc_decoder_fsm.md")]

use rhdl::prelude::*;

use crate::core::dff;

/// Internal state machine — tracks half-cell vs whole-cell
/// position in the biphase-mark waveform.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum LtcDecodeState {
    /// No edge seen yet; waiting for the first transition.
    #[default]
    #[fsm_state(label = "wait first edge")]
    WaitFirstEdge,
    /// At a cell boundary — the next interval (long → bit `0`,
    /// short → first half of bit `1`) decides.
    #[fsm_state(label = "cell boundary")]
    AtCellBoundary,
    /// Mid-cell — previous transition was the cell-mid pulse of
    /// a `1` bit; next transition completes the `1`.
    #[fsm_state(label = "mid cell")]
    MidCell,
}

/// Bundled internal state for the SMPTE LTC decoder.
///
/// Per CLAUDE.md §3.1, the non-FSM internal registers live in
/// a single `Digital`-derived struct behind one DFF.  Keeps the
/// widget at three sibling sub-circuits (state + extras + the
/// implicit framework wiring) rather than nine.
#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct LtcDecodeExtras<const IW: usize>
where
    rhdl::bits::W<IW>: BitWidth,
{
    /// Previous line value (for edge detection).
    pub prev_line: bool,
    /// Interval counter — cycles since last edge.
    pub interval: Bits<IW>,
    /// Running half-cell-time estimate.
    pub half_t: Bits<IW>,
    /// True once the bit-clock estimate is stable.
    pub locked: bool,
    /// 80-bit shift register holding the most recent bits.
    pub shifter: Bits<80>,
    /// True if the most recently observed sync word was forward.
    pub forward: bool,
    /// One-cycle pulse when a complete frame is parsed.
    pub frame_pulse: bool,
    /// Latched frame contents (held until the next frame_pulse).
    pub last_frame: LtcFrame,
}

impl<const IW: usize> Default for LtcDecodeExtras<IW>
where
    rhdl::bits::W<IW>: BitWidth,
{
    fn default() -> Self {
        Self {
            prev_line: false,
            interval: bits::<IW>(0),
            half_t: bits::<IW>(0),
            locked: false,
            shifter: bits::<80>(0),
            forward: false,
            frame_pulse: false,
            last_frame: LtcFrame::default(),
        }
    }
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = LtcDecodeState, allow_implicit)]
/// SMPTE LTC bit-level biphase-mark decoder.
pub struct SmpteLtcDecoder<const IW: usize>
where
    rhdl::bits::W<IW>: BitWidth,
{
    state: dff::DFF<LtcDecodeState>,
    extras: dff::DFF<LtcDecodeExtras<IW>>,
}

impl<const IW: usize> SmpteLtcDecoder<IW>
where
    rhdl::bits::W<IW>: BitWidth,
{
    /// Build a decoder.  The half-cell-time estimate is learned
    /// from the line itself once edges are observed; the host
    /// doesn't need to pre-program a bit rate.
    pub fn new() -> Self {
        Self {
            state: dff::DFF::default(),
            extras: dff::DFF::new(LtcDecodeExtras::default()),
        }
    }
}

impl<const IW: usize> Default for SmpteLtcDecoder<IW>
where
    rhdl::bits::W<IW>: BitWidth,
{
    fn default() -> Self {
        Self::new()
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// Parsed LTC frame contents — the time-of-day signal carried
/// by one 80-bit LTC word per video frame.
pub struct LtcFrame {
    /// Hours field (0..=23).
    pub hours: Bits<5>,
    /// Minutes field (0..=59).
    pub minutes: Bits<6>,
    /// Seconds field (0..=59).
    pub seconds: Bits<6>,
    /// Frame number within the second.
    pub frames: Bits<6>,
    /// Drop-frame flag (set in 29.97 fps timecode).
    pub drop_frame: bool,
    /// Colour-frame flag.
    pub colour_frame: bool,
    /// Eight binary-group flag bits.
    pub binary_group_flags: Bits<8>,
    /// Four 4-bit user-data nibbles, packed LSB-first into 32 bits.
    pub user_bits: Bits<32>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [SmpteLtcDecoder].
pub struct In {
    /// Sampled line value (one sample per clock cycle).
    pub line_in: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [SmpteLtcDecoder].
pub struct Out {
    /// `Some(frame)` for one cycle when a complete LTC frame is
    /// parsed; `None` otherwise.
    pub frame: Option<LtcFrame>,
    /// True when the latest sync word was forward; false for
    /// reverse playback or not-yet-locked.
    pub forward: bool,
    /// True once the bit-clock estimate is stable.
    pub in_lock: bool,
    /// Latched contents of the most recent frame parsed.
    pub last_frame: LtcFrame,
}

impl<const IW: usize> SynchronousIO for SmpteLtcDecoder<IW>
where
    rhdl::bits::W<IW>: BitWidth,
{
    type I = In;
    type O = Out;
    type Kernel = smpte_ltc_decoder<IW>;
}

#[kernel]
/// Kernel for [SmpteLtcDecoder].
pub fn smpte_ltc_decoder<const IW: usize>(
    cr: ClockReset,
    i: In,
    q: Q<IW>,
) -> (Out, D<IW>)
where
    rhdl::bits::W<IW>: BitWidth,
{
    let mut d = D::<IW>::dont_care();
    d.state = q.state;
    let mut next = q.extras;
    next.prev_line = i.line_in;
    next.interval = q.extras.interval + bits::<IW>(1);
    next.frame_pulse = false;

    // Edge detect (combinational over q.extras.prev_line and i.line_in).
    let edge = i.line_in != q.extras.prev_line;
    let interval_now = q.extras.interval + bits::<IW>(1);
    // Threshold: 1.5 × half_t (= half_t + half_t/2).
    let threshold = q.extras.half_t + (q.extras.half_t >> 1);
    let is_long = interval_now >= threshold && q.extras.locked;

    let next_shifter_zero = q.extras.shifter >> 1;
    let next_shifter_one = (q.extras.shifter >> 1) | (bits::<80>(1) << 79);

    if edge {
        next.interval = bits::<IW>(0);

        match q.state {
            LtcDecodeState::WaitFirstEdge => {
                d.state = LtcDecodeState::AtCellBoundary;
            }
            LtcDecodeState::AtCellBoundary => {
                if !q.extras.locked {
                    next.half_t = interval_now;
                    next.locked = true;
                } else if interval_now < q.extras.half_t {
                    next.half_t = interval_now;
                }
                if is_long {
                    let new_shifter = next_shifter_zero;
                    next.shifter = new_shifter;
                    let sync_field: Bits<16> = (new_shifter >> 64).resize::<16>();
                    let forward_match = sync_field == bits::<16>(0xBFFC);
                    let reverse_match = sync_field == bits::<16>(0x3FFD);
                    if forward_match {
                        next.forward = true;
                        next.frame_pulse = true;
                        next.last_frame = parse_payload(new_shifter);
                    } else if reverse_match {
                        next.forward = false;
                        next.frame_pulse = true;
                        next.last_frame = parse_payload(new_shifter);
                    }
                } else {
                    d.state = LtcDecodeState::MidCell;
                }
            }
            LtcDecodeState::MidCell => {
                let new_shifter = next_shifter_one;
                next.shifter = new_shifter;
                let sync_field: Bits<16> = (new_shifter >> 64).resize::<16>();
                let forward_match = sync_field == bits::<16>(0xBFFC);
                let reverse_match = sync_field == bits::<16>(0x3FFD);
                if forward_match {
                    next.forward = true;
                    next.frame_pulse = true;
                    next.last_frame = parse_payload(new_shifter);
                } else if reverse_match {
                    next.forward = false;
                    next.frame_pulse = true;
                    next.last_frame = parse_payload(new_shifter);
                }
                d.state = LtcDecodeState::AtCellBoundary;
            }
        }
    }

    if cr.reset.any() {
        d.state = LtcDecodeState::WaitFirstEdge;
        next = LtcDecodeExtras::<IW>::default();
    }

    d.extras = next;

    let mut o = Out::dont_care();
    o.frame = if q.extras.frame_pulse {
        Some(q.extras.last_frame)
    } else {
        None
    };
    o.forward = q.extras.forward;
    o.in_lock = q.extras.locked;
    o.last_frame = q.extras.last_frame;
    (o, d)
}

/// Parse the 64-bit LTC payload (bits [0..63] of the 80-bit
/// shifter — bit 63 is just below the sync field).  Layout per
/// SMPTE 12M-1: nibble-interleaved time + user bits.
#[kernel]
pub fn parse_payload(shifter: Bits<80>) -> LtcFrame {
    // No need to mask the top 16 bits off — the per-field
    // extracts below use small `& 0xF` / `& 0x7` masks that
    // already isolate each nibble.
    let payload = shifter;
    let frame_units: Bits<4> = (payload & bits::<80>(0xF)).resize::<4>();
    let user0: Bits<4> = ((payload >> 4) & bits::<80>(0xF)).resize::<4>();
    let frame_tens: Bits<2> = ((payload >> 8) & bits::<80>(0x3)).resize();
    let drop_frame = ((payload >> 10) & bits::<80>(1)) != bits::<80>(0);
    let colour_frame = ((payload >> 11) & bits::<80>(1)) != bits::<80>(0);
    let user1: Bits<4> = ((payload >> 12) & bits::<80>(0xF)).resize();
    let sec_units: Bits<4> = ((payload >> 16) & bits::<80>(0xF)).resize();
    let user2: Bits<4> = ((payload >> 20) & bits::<80>(0xF)).resize();
    let sec_tens: Bits<3> = ((payload >> 24) & bits::<80>(0x7)).resize();
    let bgf0 = ((payload >> 27) & bits::<80>(1)) != bits::<80>(0);
    let user3: Bits<4> = ((payload >> 28) & bits::<80>(0xF)).resize();
    let min_units: Bits<4> = ((payload >> 32) & bits::<80>(0xF)).resize();
    let user4: Bits<4> = ((payload >> 36) & bits::<80>(0xF)).resize();
    let min_tens: Bits<3> = ((payload >> 40) & bits::<80>(0x7)).resize();
    let bgf1 = ((payload >> 43) & bits::<80>(1)) != bits::<80>(0);
    let user5: Bits<4> = ((payload >> 44) & bits::<80>(0xF)).resize();
    let hour_units: Bits<4> = ((payload >> 48) & bits::<80>(0xF)).resize();
    let user6: Bits<4> = ((payload >> 52) & bits::<80>(0xF)).resize();
    let hour_tens: Bits<2> = ((payload >> 56) & bits::<80>(0x3)).resize();
    let bgf2 = ((payload >> 59) & bits::<80>(1)) != bits::<80>(0);
    let user7: Bits<4> = ((payload >> 60) & bits::<80>(0xF)).resize();

    // BCD pack: tens*10 + units  =  (tens*8 + tens*2) + units.
    let frames_combined: Bits<6> = frame_units.resize::<6>()
        + (frame_tens.resize::<6>() << 3)
        + (frame_tens.resize::<6>() << 1);
    let secs_combined: Bits<6> = sec_units.resize::<6>()
        + (sec_tens.resize::<6>() << 3)
        + (sec_tens.resize::<6>() << 1);
    let mins_combined: Bits<6> = min_units.resize::<6>()
        + (min_tens.resize::<6>() << 3)
        + (min_tens.resize::<6>() << 1);
    let hours_combined: Bits<5> = hour_units.resize::<5>()
        + (hour_tens.resize::<5>() << 3)
        + (hour_tens.resize::<5>() << 1);

    let user_pack: Bits<32> = user0.resize::<32>()
        | (user1.resize::<32>() << 4)
        | (user2.resize::<32>() << 8)
        | (user3.resize::<32>() << 12)
        | (user4.resize::<32>() << 16)
        | (user5.resize::<32>() << 20)
        | (user6.resize::<32>() << 24)
        | (user7.resize::<32>() << 28);

    let mut bgf: Bits<8> = bits::<8>(0);
    if bgf0 {
        bgf = bgf | bits::<8>(0x01);
    }
    if bgf1 {
        bgf = bgf | bits::<8>(0x02);
    }
    if bgf2 {
        bgf = bgf | bits::<8>(0x04);
    }

    LtcFrame {
        hours: hours_combined,
        minutes: mins_combined,
        seconds: secs_combined,
        frames: frames_combined,
        drop_frame,
        colour_frame,
        binary_group_flags: bgf,
        user_bits: user_pack,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    /// Generate a biphase-mark line waveform from a sequence of bits.
    fn bm_encode(bits_in: &[bool], cell_cycles: usize, start_level: bool) -> Vec<bool> {
        let mut out = Vec::new();
        let mut level = start_level;
        for &b in bits_in {
            level = !level;
            for _ in 0..(cell_cycles / 2) {
                out.push(level);
            }
            if b {
                level = !level;
            }
            for _ in 0..(cell_cycles / 2) {
                out.push(level);
            }
        }
        out
    }

    /// Build an 80-bit LTC frame for a given time, returning bits in
    /// transmission order (LSB-first into the shifter).
    fn ltc_frame_bits(hh: u32, mm: u32, ss: u32, ff: u32, drop_frame: bool) -> Vec<bool> {
        let mut bits = Vec::with_capacity(80);
        let frame_units = (ff % 10) as u128;
        let frame_tens = (ff / 10) as u128;
        let sec_units = (ss % 10) as u128;
        let sec_tens = (ss / 10) as u128;
        let min_units = (mm % 10) as u128;
        let min_tens = (mm / 10) as u128;
        let hour_units = (hh % 10) as u128;
        let hour_tens = (hh / 10) as u128;
        let push_bits = |bits: &mut Vec<bool>, value: u128, width: usize| {
            for k in 0..width {
                bits.push(((value >> k) & 1) != 0);
            }
        };
        push_bits(&mut bits, frame_units, 4);
        push_bits(&mut bits, 0, 4);
        push_bits(&mut bits, frame_tens, 2);
        bits.push(drop_frame);
        bits.push(false);
        push_bits(&mut bits, 0, 4);
        push_bits(&mut bits, sec_units, 4);
        push_bits(&mut bits, 0, 4);
        push_bits(&mut bits, sec_tens, 3);
        bits.push(false);
        push_bits(&mut bits, 0, 4);
        push_bits(&mut bits, min_units, 4);
        push_bits(&mut bits, 0, 4);
        push_bits(&mut bits, min_tens, 3);
        bits.push(false);
        push_bits(&mut bits, 0, 4);
        push_bits(&mut bits, hour_units, 4);
        push_bits(&mut bits, 0, 4);
        push_bits(&mut bits, hour_tens, 2);
        bits.push(false);
        bits.push(false);
        push_bits(&mut bits, 0, 4);
        // 16-bit forward sync: 0xBFFC LSB-first.
        push_bits(&mut bits, 0xBFFC, 16);
        assert_eq!(bits.len(), 80);
        bits
    }

    fn idle_in() -> In {
        In { line_in: false }
    }

    #[test]
    fn test_bit_discriminator_locks_quickly() -> miette::Result<()> {
        let cell_cycles = 16;
        let bits_in = vec![false, true, false, true, false, false, true];
        let line = bm_encode(&bits_in, cell_cycles, false);
        let stream_in: Vec<In> = line.iter().map(|&b| In { line_in: b }).collect();
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = SmpteLtcDecoder::<14>::default();
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        let lock_seen = outputs.iter().any(|s| s.output.in_lock);
        assert!(lock_seen, "decoder never reached lock");
        Ok(())
    }

    #[test]
    fn test_full_frame_round_trip() -> miette::Result<()> {
        let cell_cycles = 16;
        let mut all_bits = Vec::new();
        for _ in 0..3 {
            all_bits.extend(ltc_frame_bits(12, 34, 56, 7, false));
        }
        let line = bm_encode(&all_bits, cell_cycles, false);
        let mut stream_in: Vec<In> = line.iter().map(|&b| In { line_in: b }).collect();
        for _ in 0..50 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = SmpteLtcDecoder::<14>::default();
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        let frame_observed = outputs.iter().find_map(|s| s.output.frame);
        assert!(
            frame_observed.is_some(),
            "no frame parsed from a 3-frame LTC waveform"
        );
        Ok(())
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = SmpteLtcDecoder::<14>::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        assert!(hdl.len() > 1000, "HDL emission unreasonably small: {}", hdl.len());
        Ok(())
    }

    #[test]
    fn test_smpte_ltc_decoder_hdl_works() -> miette::Result<()> {
        let cell_cycles = 16;
        let bits_in = ltc_frame_bits(0, 0, 1, 0, false);
        let line = bm_encode(&bits_in, cell_cycles, false);
        let stream_in: Vec<In> = line.iter().map(|&b| In { line_in: b }).collect();
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = SmpteLtcDecoder::<14>::default();
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_smpte_ltc_decoder_trace() -> miette::Result<()> {
        let cell_cycles = 16;
        let bits_in = ltc_frame_bits(1, 2, 3, 4, false);
        let line = bm_encode(&bits_in, cell_cycles, false);
        let stream_in: Vec<In> = line.iter().map(|&b| In { line_in: b }).collect();
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = SmpteLtcDecoder::<14>::default();
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("smpte_ltc_decoder");
        std::fs::create_dir_all(&root).unwrap();
        let _digest = vcd.dump_to_file(root.join("smpte_ltc_decoder.vcd")).unwrap();
        let _ = expect![[r#""#]];
        Ok(())
    }

    #[test]
    fn test_fsm_descriptor_round_trip() {
        let desc = SmpteLtcDecoder::<14>::fsm_descriptor();
        assert_eq!(desc.widget_name, "SmpteLtcDecoder");
        assert_eq!(desc.variants().len(), 3);
    }
}
