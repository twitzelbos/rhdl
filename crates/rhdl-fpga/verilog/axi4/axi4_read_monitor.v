// A passive AXI4 read-channel protocol monitor, in plain Verilog-2001.
//
// Validation Tier B of axi4-architecture.md §7, the read-channel half.
// The write-channel half is axi4_write_monitor.v and the reasoning is
// the same: the RHDL monitor is RHDL checking RHDL, so a misreading of
// the AMBA spec shared between it and the widget under test passes
// silently. This is the independent oracle.
//
// Again deliberately implemented by other means than the Rust, so that
// the two disagree when either is wrong:
//
//   * the beats remaining are counted DOWN from ARLEN, where the Rust
//     counts elapsed beats UP and compares against ARLEN;
//   * the 4 KiB check compares the first and last byte touched, where
//     the Rust compares the pages of the first and last beat;
//   * the transaction retires on a decrement-to-zero, where the Rust
//     retires on the RLAST flag.
//
// That last one is the interesting pair: it means a burst whose RLAST
// and whose beat count disagree is seen differently by each monitor,
// which is exactly the condition RLastEarly and RLastMissing describe.
//
// Fault codes must agree with the shared Rust `Fault` enum; the Rust
// test states the mapping explicitly rather than relying on the derived
// discriminant encoding, so that list is the contract.

// The codes are a WIRE FORMAT agreed with the Rust test's `code_of`
// mapping, deliberately NOT the Rust enum's derived discriminant. That
// freedom is load-bearing: adding the read-channel variants to the
// shared `Fault` enum inserted them in the middle of the declaration
// order, which would have silently re-pointed the *write* monitor's
// existing codes at different faults. Because `code_of` is written out
// by hand, the read codes could simply be assigned above the write
// ones instead, and axi4_write_monitor.v never had to change.
//
// Write-channel codes 1..14 live in axi4_write_monitor.v. The four
// address-phase rules are shared and keep their codes there; the
// read-specific ones start at 15.
`define RF_NONE               0
`define RF_INCR_CROSSES_4K   10
`define RF_WRAP_BAD_LENGTH   11
`define RF_WRAP_UNALIGNED    12
`define RF_SIZE_EXCEEDS_BUS  13
`define RF_AR_VALID_DROPPED  15
`define RF_AR_PAYLOAD_CHANGED 16
`define RF_R_VALID_DROPPED   17
`define RF_R_PAYLOAD_CHANGED 18
`define RF_R_LAST_EARLY      19
`define RF_R_LAST_MISSING    20
`define RF_R_BEAT_UNEXPECTED 21
`define RF_R_ID_MISMATCH     22

module axi4_read_monitor (
    input  wire        clk,
    input  wire        rstn,          // active low, AMBA style
    input  wire        ar_valid,
    input  wire        ar_ready,
    input  wire [ 3:0] ar_id,
    input  wire [31:0] ar_addr,
    input  wire [ 7:0] ar_len,
    input  wire [ 2:0] ar_size,
    input  wire [ 1:0] ar_burst,
    input  wire        r_valid,
    input  wire        r_ready,
    input  wire [ 3:0] r_id,
    input  wire [31:0] r_data,
    input  wire [ 1:0] r_resp,
    input  wire        r_last,
    output reg  [ 4:0] fault,
    output reg  [15:0] faults,
    output reg  [15:0] unchecked
);

  localparam BUS_BYTES   = 4;   // a 32-bit data bus
  localparam BURST_INCR  = 2'b01;
  localparam BURST_WRAP  = 2'b10;

  reg        seen;
  reg        p_ar_valid, p_ar_ready;
  reg [ 3:0] p_ar_id;
  reg [31:0] p_ar_addr;
  reg [ 7:0] p_ar_len;
  reg [ 2:0] p_ar_size;
  reg [ 1:0] p_ar_burst;
  reg        p_r_valid, p_r_ready, p_r_last;
  reg [ 3:0] p_r_id;
  reg [31:0] p_r_data;
  reg [ 1:0] p_r_resp;

  // One slot per ARID, as three parallel memory arrays. The Rust keeps
  // a `Digital` array of structs; what differs is that this counts each
  // ID's beats UP against its ARLEN while the Rust compares against
  // `burst_beats(len) - 1`. Same predicate, other arithmetic.
  reg        slot_active [0:15];
  reg [ 7:0] slot_len    [0:15];
  reg [ 7:0] slot_beat   [0:15];
  reg        any_active;
  integer    si;

  reg [31:0] first_byte, last_byte, span, aligned;

  reg ar_held, ar_dropped, ar_changed;
  reg r_held,  r_dropped,  r_changed;
  reg ar_fire, r_fire, checked, is_wrap;
  reg size_bad, crosses, wrap_len_bad, wrap_unaligned;
  reg last_early, last_missing, id_mismatch, beat_unexpected;
  integer nhits;

  task note;                      // sticky-first latch, plus a tally
    input [4:0] code;
    input       cond;
    begin
      if (cond) begin
        nhits = nhits + 1;
        if (fault == `RF_NONE) fault = code;
      end
    end
  endtask

  always @(posedge clk) begin
    if (!rstn) begin
      fault <= `RF_NONE; faults <= 0; unchecked <= 0;
      seen <= 0;
      for (si = 0; si < 16; si = si + 1) begin
        slot_active[si] <= 0;
        slot_len[si]    <= 0;
        slot_beat[si]   <= 0;
      end
      p_ar_valid <= 0; p_ar_ready <= 0; p_r_valid <= 0; p_r_ready <= 0;
    end else begin
      nhits = 0;

      // ---- handshake stability
      ar_held    = seen && p_ar_valid && !p_ar_ready;
      ar_dropped = ar_held && !ar_valid;
      ar_changed = ar_held && ar_valid &&
                   ({ar_id, ar_addr, ar_len, ar_size, ar_burst} !=
                    {p_ar_id, p_ar_addr, p_ar_len, p_ar_size, p_ar_burst});
      r_held     = seen && p_r_valid && !p_r_ready;
      r_dropped  = r_held && !r_valid;
      r_changed  = r_held && r_valid &&
                   ({r_id, r_data, r_resp, r_last} !=
                    {p_r_id, p_r_data, p_r_resp, p_r_last});

      // ---- address phase, the rules AW and AR share
      ar_fire  = ar_valid && ar_ready;
      is_wrap  = (ar_burst == BURST_WRAP);
      size_bad = ar_fire && ((1 << ar_size) > BUS_BYTES);

      aligned    = ar_addr & ~((1 << ar_size) - 1);
      span       = (ar_len + 1) << ar_size;
      first_byte = aligned;
      last_byte  = aligned + span - 1;
      crosses = ar_fire && (ar_burst == BURST_INCR) &&
                ((first_byte >> 12) != (last_byte >> 12));

      wrap_len_bad = ar_fire && is_wrap &&
                     !(ar_len == 1 || ar_len == 3 || ar_len == 7 || ar_len == 15);
      wrap_unaligned = ar_fire && is_wrap &&
                       ((ar_addr & ((1 << ar_size) - 1)) != 0);

      // ---- data phase. `remaining` counts beats still owed, so the
      // final beat is the one where it reaches 1.
      r_fire  = r_valid && r_ready;
      // Does anything at all have a transaction in flight? A beat
      // arriving on an idle bus is a different mistake from one naming
      // the wrong ID while other transactions legitimately progress.
      any_active = 1'b0;
      for (si = 0; si < 16; si = si + 1)
        if (slot_active[si]) any_active = 1'b1;

      checked = r_fire && slot_active[r_id];
      // Counted UP against ARLEN: the final beat is the one whose index
      // equals it. Read data for different IDs may interleave beat by
      // beat, so each ID's own counter is what sequences it; a single
      // counter cannot tell a legal interleave from a reordered burst.
      last_early   = checked && r_last  && (slot_beat[r_id] != slot_len[r_id]);
      last_missing = checked && !r_last && (slot_beat[r_id] == slot_len[r_id]);
      id_mismatch  = r_fire && !slot_active[r_id] && any_active;
      beat_unexpected = r_fire && !slot_active[r_id] && !any_active;

      note(`RF_AR_VALID_DROPPED,   ar_dropped);
      note(`RF_AR_PAYLOAD_CHANGED, ar_changed);
      note(`RF_R_VALID_DROPPED,    r_dropped);
      note(`RF_R_PAYLOAD_CHANGED,  r_changed);
      note(`RF_SIZE_EXCEEDS_BUS,   size_bad);
      note(`RF_INCR_CROSSES_4K,    crosses);
      note(`RF_WRAP_BAD_LENGTH,    wrap_len_bad);
      note(`RF_WRAP_UNALIGNED,     wrap_unaligned);
      note(`RF_R_LAST_EARLY,       last_early);
      note(`RF_R_LAST_MISSING,     last_missing);
      note(`RF_R_ID_MISMATCH,      id_mismatch);
      note(`RF_R_BEAT_UNEXPECTED,  beat_unexpected);

      faults    <= faults + nhits;
      // A beat for an inactive ID, or a second AR for an ID already in
      // flight: legal AXI4 in the second case, but one slot per ID
      // cannot sequence two of them, so it is counted rather than
      // silently mis-tracked.
      unchecked <= unchecked
                 + ((r_fire && !slot_active[r_id]) ? 1 : 0)
                 + ((ar_fire && slot_active[ar_id]) ? 1 : 0);

      seen <= 1'b1;
      p_ar_valid <= ar_valid; p_ar_ready <= ar_ready;
      p_ar_id <= ar_id; p_ar_addr <= ar_addr; p_ar_len <= ar_len;
      p_ar_size <= ar_size; p_ar_burst <= ar_burst;
      p_r_valid <= r_valid; p_r_ready <= r_ready;
      p_r_id <= r_id; p_r_data <= r_data; p_r_resp <= r_resp;
      p_r_last <= r_last;

      // ---- the burst in flight. It retires on RLAST, matching the
      // Rust, because retiring on the countdown instead would make a
      // missing RLAST unobservable: the transaction would close and the
      // next beat would look like a surplus rather than naming the real
      // rule. The countdown is the *position* oracle, not the lifetime.
      // Activate on AR, advance or retire on R -- each ID independently,
      // so an AR and an R naming different IDs in one cycle both land.
      if (ar_fire && !slot_active[ar_id]) begin
        slot_active[ar_id] <= 1'b1;
        slot_len[ar_id]    <= ar_len;
        slot_beat[ar_id]   <= 0;
      end
      if (checked) begin
        if (r_last) begin
          slot_active[r_id] <= 1'b0;
          slot_beat[r_id]   <= 0;
        end else begin
          slot_beat[r_id] <= slot_beat[r_id] + 1;
        end
      end
    end
  end
endmodule
