// A passive AXI4 write-channel protocol monitor, in plain Verilog-2001.
//
// Validation Tier B of axi4-architecture.md §7, and the reason there is
// a Tier B at all: Tier A's monitor is RHDL checking RHDL, so a
// misreading of the AMBA spec shared between the monitor and the widget
// under test passes silently. This file is the independent oracle. It is
// written from the specification by hand and is deliberately *not* a
// transliteration of the Rust -- where there is a choice of
// implementation it takes the other one, so that the two disagree when
// either is wrong:
//
//   * the legal strobe mask is built by a loop over byte lanes, where
//     the Rust builds it by shifting a run of ones;
//   * the beat address is accumulated beat by beat, where the Rust
//     computes it closed-form from the beat index;
//   * the 4 KiB check compares the first and last byte touched, where
//     the Rust compares the pages of the first and last beat.
//
// Agreement between two implementations that made different choices is
// evidence; agreement between a function and its own copy is not.
//
// Verilog-2001 only: iverilog 13 is the toolchain and it has no SVA, so
// the assertions are ordinary always-blocks rather than `assert
// property`.
//
// Fault codes are listed below and must agree with the Rust `Fault`
// enum. The Rust test states the mapping explicitly rather than relying
// on the derived discriminant encoding, so this list is the contract.

`define F_NONE                 0
`define F_AW_VALID_DROPPED     1
`define F_AW_PAYLOAD_CHANGED   2
`define F_W_VALID_DROPPED      3
`define F_W_PAYLOAD_CHANGED    4
`define F_B_VALID_DROPPED      5
`define F_B_PAYLOAD_CHANGED    6
`define F_W_LAST_EARLY         7
`define F_W_LAST_MISSING       8
`define F_BRESP_UNEXPECTED     9
`define F_INCR_CROSSES_4K     10
`define F_WRAP_BAD_LENGTH     11
`define F_WRAP_UNALIGNED      12
`define F_SIZE_EXCEEDS_BUS    13
`define F_STROBE_OUTSIDE_LANES 14

module axi4_write_monitor (
    input  wire        clk,
    input  wire        rstn,          // active low, AMBA style
    input  wire        aw_valid,
    input  wire        aw_ready,
    input  wire [ 3:0] aw_id,
    input  wire [31:0] aw_addr,
    input  wire [ 7:0] aw_len,
    input  wire [ 2:0] aw_size,
    input  wire [ 1:0] aw_burst,
    input  wire        w_valid,
    input  wire        w_ready,
    input  wire [31:0] w_data,
    input  wire [ 3:0] w_strb,
    input  wire        w_last,
    input  wire        b_valid,
    input  wire        b_ready,
    input  wire [ 3:0] b_id,
    input  wire [ 1:0] b_resp,
    output reg  [ 4:0] fault,
    output reg  [15:0] faults,
    output reg  [15:0] unchecked
);

  localparam BUS_BYTES = 4;   // a 32-bit data bus
  localparam BURST_FIXED = 2'b00;
  localparam BURST_INCR  = 2'b01;
  localparam BURST_WRAP  = 2'b10;

  // Previous cycle, for the handshake-stability rules.
  reg        seen;
  reg        p_aw_valid, p_aw_ready;
  reg [ 3:0] p_aw_id;
  reg [31:0] p_aw_addr;
  reg [ 7:0] p_aw_len;
  reg [ 2:0] p_aw_size;
  reg [ 1:0] p_aw_burst;
  reg        p_w_valid, p_w_ready, p_w_last;
  reg [31:0] p_w_data;
  reg [ 3:0] p_w_strb;
  reg        p_b_valid, p_b_ready;
  reg [ 3:0] p_b_id;
  reg [ 1:0] p_b_resp;

  // The burst in flight.
  reg        aw_active;
  reg [31:0] cur_addr;      // accumulated, not recomputed
  reg [31:0] blk_lo;        // wrap block base
  reg [31:0] blk_mask;
  reg [ 7:0] cur_len;
  reg [ 2:0] cur_size;
  reg [ 1:0] cur_burst;
  reg [ 7:0] beat;
  reg [ 7:0] outstanding;

  integer    li;
  reg [ 3:0] legal;
  reg [31:0] tb;            // bytes this transfer
  reg [31:0] start_addr;    // AWADDR as issued, for the reduced first beat
  reg [31:0] base_lo;       // low lane of the beat's size-aligned block
  reg [31:0] start_lo;      // low lane the start address actually begins at
  reg [31:0] lo;            // first lane of the transfer within the bus
  reg [31:0] first_byte, last_byte, span;
  reg [31:0] aligned;

  // Combinational predicates for this cycle.
  reg aw_held, aw_dropped, aw_changed;
  reg w_held,  w_dropped,  w_changed;
  reg b_held,  b_dropped,  b_changed;
  reg aw_fire, w_fire, b_fire, checked, is_wrap;
  reg size_bad, crosses, wrap_len_bad, wrap_unaligned;
  reg last_early, last_missing, strobe_bad, bresp_bad, size_ok;
  reg [4:0] hit;
  integer   nhits;

  task note;                      // sticky-first latch, plus a tally
    input [4:0] code;
    input       cond;
    begin
      if (cond) begin
        nhits = nhits + 1;
        if (fault == `F_NONE) fault = code;
      end
    end
  endtask

  always @(posedge clk) begin
    if (!rstn) begin
      fault <= `F_NONE; faults <= 0; unchecked <= 0;
      seen <= 0; aw_active <= 0; beat <= 0; outstanding <= 0;
      p_aw_valid <= 0; p_aw_ready <= 0; p_w_valid <= 0; p_w_ready <= 0;
      p_b_valid <= 0; p_b_ready <= 0;
    end else begin
      nhits = 0;

      // ---- handshake stability
      aw_held    = seen && p_aw_valid && !p_aw_ready;
      aw_dropped = aw_held && !aw_valid;
      aw_changed = aw_held && aw_valid &&
                   ({aw_id, aw_addr, aw_len, aw_size, aw_burst} !=
                    {p_aw_id, p_aw_addr, p_aw_len, p_aw_size, p_aw_burst});
      w_held     = seen && p_w_valid && !p_w_ready;
      w_dropped  = w_held && !w_valid;
      w_changed  = w_held && w_valid &&
                   ({w_data, w_strb, w_last} != {p_w_data, p_w_strb, p_w_last});
      b_held     = seen && p_b_valid && !p_b_ready;
      b_dropped  = b_held && !b_valid;
      // BRESP is part of the payload, not just the ID. Leaving it out
      // here made Tier B blind to a response whose status changed while
      // the beat was offered -- which the Tier A/Tier B agreement test
      // is precisely what caught.
      b_changed  = b_held && b_valid && ({b_id, b_resp} != {p_b_id, p_b_resp});

      // ---- address-phase legality, as the AW is accepted
      aw_fire = aw_valid && aw_ready;
      is_wrap = (aw_burst == BURST_WRAP);
      size_bad = aw_fire && ((1 << aw_size) > BUS_BYTES);

      // 4 KiB: compare the first and the LAST BYTE touched, which is a
      // different formulation from the Rust's page-of-the-last-beat.
      aligned    = aw_addr & ~((1 << aw_size) - 1);
      span       = (aw_len + 1) << aw_size;
      first_byte = aligned;
      last_byte  = aligned + span - 1;
      crosses = aw_fire && (aw_burst == BURST_INCR) &&
                ((first_byte >> 12) != (last_byte >> 12));

      wrap_len_bad = aw_fire && is_wrap &&
                     !(aw_len == 1 || aw_len == 3 || aw_len == 7 || aw_len == 15);
      wrap_unaligned = aw_fire && is_wrap &&
                       ((aw_addr & ((1 << aw_size) - 1)) != 0);

      // ---- data phase
      w_fire  = w_valid && w_ready;
      checked = w_fire && aw_active;
      last_early   = checked && w_last  && (beat != cur_len);
      last_missing = checked && !w_last && (beat == cur_len);

      // ---- strobe lanes, built by walking the lanes
      // An oversized AxSIZE is its own fault, and the lane check is
      // SUPPRESSED rather than clamped when it occurs: a clamp is an
      // arbitrary choice, and this monitor and the Rust one originally
      // made opposite ones -- full bus here, one byte there -- so the
      // same illegal burst drew a different fault count from each. See
      // the comment in monitor.rs.
      size_ok = ((1 << cur_size) <= BUS_BYTES);
      tb = size_ok ? (1 << cur_size) : BUS_BYTES;
      base_lo = (cur_addr & (BUS_BYTES - 1)) & ~(tb - 1);
      // An unaligned burst's FIRST beat is narrower than its size: it
      // runs from AWADDR to the end of that size-aligned block, and the
      // lanes below the start address stay dark. Later beats are
      // size-aligned by construction. Kept as a lane walk rather than
      // the Rust's mask arithmetic -- the point of this file is that it
      // reaches the same answer by other means.
      start_lo = start_addr & (BUS_BYTES - 1);
      lo = ((beat == 0) && (start_lo > base_lo)) ? start_lo : base_lo;
      legal = 0;
      for (li = 0; li < BUS_BYTES; li = li + 1)
        if ((li >= lo) && (li < base_lo + tb)) legal[li] = 1'b1;
      strobe_bad = checked && size_ok && ((w_strb & ~legal) != 0);

      // ---- response phase
      b_fire = b_valid && b_ready;
      bresp_bad = b_fire && (outstanding == 0);

      note(`F_AW_VALID_DROPPED,     aw_dropped);
      note(`F_AW_PAYLOAD_CHANGED,   aw_changed);
      note(`F_W_VALID_DROPPED,      w_dropped);
      note(`F_W_PAYLOAD_CHANGED,    w_changed);
      note(`F_B_VALID_DROPPED,      b_dropped);
      note(`F_B_PAYLOAD_CHANGED,    b_changed);
      note(`F_SIZE_EXCEEDS_BUS,     size_bad);
      note(`F_INCR_CROSSES_4K,      crosses);
      note(`F_WRAP_BAD_LENGTH,      wrap_len_bad);
      note(`F_WRAP_UNALIGNED,       wrap_unaligned);
      note(`F_W_LAST_EARLY,         last_early);
      note(`F_W_LAST_MISSING,       last_missing);
      note(`F_STROBE_OUTSIDE_LANES, strobe_bad);
      note(`F_BRESP_UNEXPECTED,     bresp_bad);

      faults    <= faults + nhits;
      unchecked <= unchecked + ((w_fire && !aw_active) ? 1 : 0);

      // ---- registered history
      seen <= 1'b1;
      p_aw_valid <= aw_valid; p_aw_ready <= aw_ready;
      p_aw_id <= aw_id; p_aw_addr <= aw_addr; p_aw_len <= aw_len;
      p_aw_size <= aw_size; p_aw_burst <= aw_burst;
      p_w_valid <= w_valid; p_w_ready <= w_ready;
      p_w_data <= w_data; p_w_strb <= w_strb; p_w_last <= w_last;
      p_b_valid <= b_valid; p_b_ready <= b_ready; p_b_id <= b_id;
      p_b_resp <= b_resp;

      // ---- the burst in flight. The address is ACCUMULATED here,
      // where the Rust recomputes it from the beat index.
      if (aw_fire) begin
        aw_active <= 1'b1;
        cur_addr  <= aw_addr & ~((1 << aw_size) - 1);
        start_addr <= aw_addr;
        start_addr <= aw_addr;
        cur_len   <= aw_len;
        cur_size  <= aw_size;
        cur_burst <= aw_burst;
        blk_mask  <= ((aw_len + 1) << aw_size) - 1;
        blk_lo    <= aw_addr & ~(((aw_len + 1) << aw_size) - 1);
        beat      <= 0;
      end else if (checked && w_last) begin
        aw_active <= 1'b0;
        beat      <= 0;
      end else if (checked) begin
        beat <= beat + 1;
        case (cur_burst)
          BURST_FIXED: cur_addr <= cur_addr;
          BURST_WRAP:  cur_addr <= blk_lo | ((cur_addr + tb) & blk_mask);
          default:     cur_addr <= cur_addr + tb;
        endcase
      end

      // Outstanding transactions, not underflowing past zero: a
      // response with nothing outstanding is reported above and must
      // not wrap the counter round to swallow later checks.
      if (aw_fire && !(b_fire && outstanding != 0))      outstanding <= outstanding + 1;
      else if (!aw_fire && (b_fire && outstanding != 0))  outstanding <= outstanding - 1;
    end
  end
endmodule
