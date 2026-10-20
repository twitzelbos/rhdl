# RoCEv2 — RDMA over Converged Ethernet v2 for RHDL

**Status: scope document. No code written. The staging in §9 needs explicit
sign-off before Phase 0 starts** — see §12.

RoCEv2 lets a remote machine's network card write directly into this
machine's memory, and vice versa, with no CPU on either end in the data
path. On an FPGA that means an instrument can deposit acquired samples
into a host application's buffer at line rate, with no driver, no socket,
no copy, and no interrupt per transfer.

This document scopes what it would take to build that in RHDL. It is a
plan, not a proposal to start tomorrow: the honest total for a full
Reliable Connection implementation is **13–18 months for one engineer**,
and the useful subset is **4.5–6 months**. Both numbers include the
dependencies RHDL does not have yet, which are most of the near-term
cost.

---

## 1 — Why this, why now, and the honest caveat

### The utilitarian case

Every data-acquisition instrument eventually hits the same wall: the
samples are in the FPGA and the science is in host memory, and the path
between them is the bottleneck and the jitter source. The usual answers
are a vendor DMA engine over PCIe (fast, but ties the instrument to a
slot in one chassis) or a socket from a soft/hard CPU (flexible, but
costs a core and adds milliseconds of tail latency).

RDMA is the third answer: the instrument becomes a peer on an Ethernet
fabric and writes into the host's address space directly. It is the
transport the CTAO-LST camera readout chose, it is what FERoCE at CERN
was built for, and it is what every modern storage and ML fabric runs on.
For an NMR/MRI console — the use case driving the `dsp::` track in this
repository — it removes the host CPU from the acquisition path entirely.

### The strategic case for RHDL specifically

RoCEv2 is a *stack of typed headers*. That is the thing RHDL is best at
and the thing `stream-bus-architecture.md` was written to complain about:
AXI4-Stream carries an untyped `TDATA` with out-of-band `TKEEP`/`TLAST`
magic, and every existing RoCEv2 implementation spends real effort on
bit-twiddling header fields in and out of a flat bus. In RHDL a Base
Transport Header is a `#[derive(Digital)]` struct, the opcode is an enum
whose illegal encodings do not exist, and the packet is an `RCStream` of
a sum type. The protocol *wants* to be written this way.

Second, and more pointedly: **the most complete open-source RoCEv2 core
is written in Bluespec** (§3). RHDL now ships `rhdl-rule` — Bluespec-style
guarded atomic rules, 7.6 kLOC across three crates with ~20 integration
tests and three pilot widgets. A RoCEv2 core is the canonical workload
for that extension, and porting a known-good Bluespec design is a
*translation* exercise with a reference oracle rather than a
from-scratch protocol implementation. That combination — hard real
protocol, existing BSV reference, RHDL's rule layer ready — is a
strong publication and the best available proof that `rhdl-rule` was
worth building.

### The caveat, stated plainly

This is a **Tier-C-scale project** in the sense of
`tier-c-flagship-cores.md`: comparable in effort to RV32I or the Alto,
and to be scheduled against them, not alongside the Tier 0–4 widget
library. It is not a widget. It is not a month of work. Anyone reading
§9 and starting at Phase 2 because Phase 0 looks boring will produce a
stack that cannot compute a valid ICRC and cannot get a frame onto a
wire.

There is also a hardware-reality caveat. RoCEv2's Reliable Connection
transport assumes a *lossless* fabric — that is what the "Converged
Ethernet" in the name means, and it is why production deployments need
PFC and DCQCN on managed switches. A point-to-point link from an
instrument to a host has no switch, no congestion and essentially no
loss, which is exactly why the staging in §5 starts with the
unreliable-connected transport: on a direct link it is not a compromise.

---

## 2 — What RoCEv2 is on the wire

A RoCEv2 packet is an ordinary UDP datagram on port **4791** whose payload
is an InfiniBand transport packet:

```text
+----------+-----------+----------+---------+-----------+---------+------+-----+
| Eth MAC  | IPv4 / v6 |   UDP    | IB BTH  | ext. hdrs | payload | ICRC | FCS |
|  14 B    | 20 / 40 B |   8 B    |  12 B   | 0/4/16 B  |  0..PMTU|  4 B | 4 B |
+----------+-----------+----------+---------+-----------+---------+------+-----+
                       dport=4791                                  ^
                                                                   |
                           CRC32 over the whole packet, with mutable fields masked
```

Everything left of the BTH is ordinary networking. Everything right of
it is InfiniBand, and that is where the work is.

### Base Transport Header (12 bytes)

Field layout, confirmed against Scapy's `contrib/roce.py`:

| Field | Bits | Notes |
|---|---|---|
| `opcode` | 8 | transport service (3 msb) + operation (5 lsb) |
| `solicited` | 1 | solicited event |
| `migreq` | 1 | migration request |
| `padcount` | 2 | payload pad bytes, 0–3 — the ragged-tail mechanism |
| `version` | 4 | transport header version |
| `pkey` | 16 | partition key, `0xffff` for a single-partition fabric |
| `fecn` | 1 | forward ECN — **masked for ICRC** |
| `becn` | 1 | backward ECN — **masked for ICRC** |
| `resv6` | 6 | **masked for ICRC** |
| `dqpn` | 24 | destination queue pair number |
| `ackreq` | 1 | requester wants an ACK for this packet |
| `resv7` | 7 | |
| `psn` | 24 | packet sequence number |

The opcode space is a transport base plus an operation offset:

| Transport | Base | Operations available |
|---|---|---|
| RC — Reliable Connection | `0x00` | all, `0x00`–`0x14` |
| UC — Unreliable Connection | `0x20` | SEND and RDMA WRITE only, `0x00`–`0x0B` |
| RD — Reliable Datagram | `0x40` | all |
| UD — Unreliable Datagram | `0x60` | SEND ONLY and SEND ONLY WITH IMMEDIATE only |

Operation offsets run SEND FIRST/MIDDLE/LAST/ONLY and their immediate
variants, then RDMA WRITE FIRST/MIDDLE/LAST/ONLY and immediate variants,
then RDMA READ REQUEST, READ RESPONSE FIRST/MIDDLE/LAST/ONLY,
ACKNOWLEDGE (`0x11`), ATOMIC ACKNOWLEDGE (`0x12`), COMPARE SWAP
(`0x13`), FETCH ADD (`0x14`). A Congestion Notification Packet is the
out-of-band opcode `0x81`.

**This table is the argument for building it in RHDL.** "UC supports only
SEND and RDMA WRITE" is a sentence in a specification that every other
implementation enforces by hand, or does not enforce. In RHDL it is a
type: an enum per transport whose variants are the legal operations, and
a UC queue pair that cannot be handed a READ because no such value
exists.

### Extended transport headers

- **RETH** (16 B) — on the first or only packet of an RDMA WRITE, and on
  an RDMA READ REQUEST: remote virtual address (64), R_Key (32), DMA
  length (32).
- **AETH** (4 B) — on ACK/NAK and READ RESPONSE: syndrome (8), message
  sequence number (24). The syndrome encodes ACK-with-credit, RNR-NAK
  with a timer, and the NAK codes (PSN sequence error, invalid request,
  remote access error, remote operation error).
- **ImmDt** (4 B) — immediate data on the `_WITH_IMMEDIATE` variants.

### Segmentation

A message larger than the path MTU (256/512/1024/2048/4096 bytes) is
split across packets: FIRST, zero or more MIDDLEs, LAST — or a single
ONLY. Only FIRST/ONLY carry the RETH; the receiver tracks the running
address. The final packet's `padcount` covers the ragged tail, since the
payload is padded to a 4-byte boundary.

Segmentation plus `padcount` is the whole variable-length problem, and
§8 argues it is the biggest language-level risk in this project.

---

## 3 — Reference implementations

The user's note that "there are sample implementations on GitHub" is
correct and they are unusually good. Surveyed:

| Project | Language | Scope | Use to us |
|---|---|---|---|
| **[datenlord/blue-rdma](https://github.com/datenlord/blue-rdma)** | Bluespec SV | Full RC: send/receive queues, retry, duplicate-read cache, work completions | **The primary reference.** Bluespec ≈ `rhdl-rule`. Gives us a module decomposition that is known to work and a semantics that translates. |
| **[fpgasystems/fpga-network-stack](https://github.com/fpgasystems)** + **RoCE BALBOA** | Vitis HLS | 100G, RDMA WRITE + READ, HBM-backed retransmission, DCQCN, Coyote shell | Architecture reference for the *pipeline shape* and for what congestion control costs. Tied to Coyote and to HBM boards. |
| **[ETH-PBL/RoCKS](https://github.com/ETH-PBL/RoCKS)** | Verilog + HLS core | RoCEv2 on a Kria KR260, bundles verilog-ethernet and a Linux driver | The *integration* reference: what the host side and the board plumbing look like. GPL-3.0 — read, do not copy. |
| **[CodeByRachit/Lightweight-RoCEv2-Stack](https://github.com/CodeByRachit/Lightweight-RoCEv2-Stack)** (FERoCE) | SystemVerilog | TX-only RC WRITE/SEND, minimal RX for ACK/NAK/CNP, URAM/HBM retransmit buffer, UDP connection manager, 10G–400G | **The scope reference for our Phase 2–3.** Proves a deliberately minimal endpoint is a real, publishable, deployed thing. Ships Wireshark dissectors and a Rust reference receiver (`feroce-rs`). |
| **[secdev/scapy](https://github.com/secdev/scapy/blob/master/scapy/contrib/roce.py)** `contrib/roce.py` | Python | Full header definitions + working ICRC | **The test oracle.** See §10. GPL-2.0: we use it as an external vector generator, we do not vendor it. |

Two measurements worth writing down, because they set expectations:

- **blue-rdma's `src/` is ~909 KB of Bluespec across 22 files** —
  roughly 26 kLOC. Of that, `ReqHandleRQ.bsv` alone is 160 KB,
  `RespHandleSQ.bsv` 74 KB, `RetryHandleSQ.bsv` 28 KB. **Nearly a third
  of the implementation is the reliable-transport state machinery** —
  receive-queue request handling, send-queue response handling, retry.
  That is the part §5 defers, and the file sizes are why.
- **BALBOA needs HBM for its retransmission buffer** at 100G. A
  retransmission buffer must hold one bandwidth-delay product; at 100G
  and 10 µs that is 125 KB and climbing, at 1 GbE and 100 µs it is
  12.5 KB. **1 GbE is the rate at which this project fits in block RAM**,
  which is a strong independent reason to start there.

Licensing: blue-rdma, RoCKS and Scapy are copyleft (GPL-family). We read
them for architecture and use them as oracles. No code is copied. The
IBTA specification itself is the normative source for field layouts, and
`tier-c-flagship-cores.md` §3.4's "specification rewrite" discipline
applies: the deliverable includes our own prose statement of the subset
we implement.

---

## 4 — What RHDL has, and the three things it does not

Verified against `main` at the time of writing.

### Already in the tree and directly reusable

- **`rcstream::*`** — the typed latency-insensitive bus, with `chunked`
  and `flatten` (element stream ↔ fixed-size arrays — exactly the
  beat/packet boundary conversion a MAC datapath needs), `relay`,
  `credit`, `cdc`, `fanout`, `axi_stream` interop, and a `testing`
  fixture. This is a large head start.
- **`core::ram::{synchronous, asynchronous, option_sync, option_async,
  pipe_sync}`** — the retransmission buffer, the packet FIFO and the
  queue-pair context table are all RAMs.
- **`cdc::*`** — a MAC's receive clock is recovered from the wire and is
  not the system clock. This is the one genuinely asynchronous part of
  the design and the `Circuit` (not `Synchronous`) family exists for it.
- **`fifo::*`**, including `AsyncFIFO`, for the clock-domain crossing on
  the datapath itself.
- **`rhdl-rule`** (7.6 kLOC, mature: `urgent_before`, `conflict_free`,
  conflict-matrix diagnostics, pilot widgets). The reference
  implementation's idiom, available natively.
- **`serial_bus::can_*`** as the worked example of a protocol PHY at
  this scale, and **CLAUDE.md §3.1**'s single-FSM-DFF + bundled-extras
  pattern, which exists precisely because protocol state machines blow
  past the 12-element `Q`/`D` tuple ceiling. Every state machine in this
  project will use it.

### The three gaps — and they are the near-term cost

**1. There is no Ethernet MAC.** `widget-roadmap.md` item 59 — "Ethernet
MAC frontend | The killer demo. Composes (16) CRC32, framing logic, CDC"
— is unshipped. Nothing in `crates/rhdl-fpga/src/` mentions Ethernet,
GMII or RGMII. This is a hard dependency: without it there is no wire.

**2. The CRC engine is bit-serial and cannot be used.**
`core::crc::CrcEngine<W>` processes **one bit per cycle**, MSB-first, and
its own module documentation says reflection and final XOR-out are
deliberately left out for the caller to apply at the message boundary.
Both the Ethernet FCS and the RoCEv2 ICRC are CRC-32 with polynomial
`0x04C11DB7`, reflected, and both must run at datapath width — 8
bits/cycle at 1 GbE, 64 at 10G. A bit-serial engine at 1 GbE would need
to run at 1 GHz. **A parallel CRC-32, generic over bytes-per-cycle and
with a byte-enable for the ragged final beat, is a prerequisite**, and it
is a genuinely useful widget on its own that the roadmap already wants.

**3. There is no IPv4 or UDP header layer.** Small, but real: header
construction, the IPv4 header checksum, and — for the ICRC — the ability
to re-serialise those headers with specific fields masked.

### A fourth gap that is not RHDL's problem but is ours

No board in `rhdl-bsp` has a high-speed transceiver; the supported
targets are the OpalKelly XEM7010 (Artix-7) and Lattice parts. There is
no 10G-capable board support, which reinforces 1 GbE as the starting
rate. And for the instrument use case specifically: on a Red Pitaya the
Ethernet PHY is wired to the Zynq PS's hard GEM, not to the PL — so the
PL does not own a MAC and RoCEv2 is **not** a drop-in there. Confirm
against the board schematic before planning any hardware bring-up; this
may well mean the first real target is a different board.

---

## 5 — Scope decision: UC RDMA WRITE first

**Decision recorded: the first endpoint that moves data is UC
(Unreliable Connection) RDMA WRITE, transmit side, at 1 GbE.** RC,
READ, SEND/RECV, atomics and congestion control are later phases,
explicitly staged, not silently dropped.

CLAUDE.md's anti-sliver rule requires justifying this rather than
asserting it, because "ship the easy transport first" is exactly the
shape that rule forbids. The justification:

1. **UC RDMA WRITE is the operation the use case needs.** An instrument
   streaming acquisition data into a pre-registered host buffer is a
   one-directional WRITE. It never issues a READ, never needs an
   incoming SEND, and has no use for atomics.
2. **It is a complete, externally-verifiable deliverable, not a
   fraction of one.** A UC WRITE endpoint interoperates with an
   unmodified Linux RDMA stack. It can be pointed at a real host, and
   data appears in a real application's memory. That is a whole thing
   that works, which is the test the anti-sliver rule actually applies.
3. **UC is not a degraded RC on this topology.** On a direct
   instrument-to-host link there is no switch to drop frames; UC's lack
   of acknowledgement costs nothing that the fabric was going to provide
   anyway. On a shared, congested fabric this would be the wrong choice
   and RC's absence would be a real gap — §11 records that.
4. **It defers precisely the third of the problem that the reference
   implementation's file sizes identify as the expensive third** (§3),
   and nothing else.
5. **The precedent exists**: FERoCE is a deployed, published, TX-only
   endpoint.

What this explicitly does *not* license: calling Phase 2 "RoCEv2
support" without qualification, or opening Phase 3 before Phase 2 is
interoperable against real hardware. The CHANGELOG entry for Phase 2
names the transport and the direction in its title.

---

## 6 — Architecture

### Crate layout

A separate crate, following the `rhdl-rv32i` / `rhdl-alto` precedent for
project-scale work that is not a widget:

```text
crates/rhdl-roce/
  Cargo.toml
  README.md
  roce-subset-spec.md        our normative statement of the implemented subset
  src/
    lib.rs
    headers/                 BTH, RETH, AETH, ImmDt as Digital types
      opcode.rs              transport-and-operation enums; illegal states absent
      bth.rs  reth.rs  aeth.rs
      serde.rs               to/from a byte-beat stream, width-generic
    icrc.rs                  the masked-field CRC32 (see §7)
    qp/
      context.rs             queue-pair context table (RAM-backed)
      uc_write_sq.rs         UC send queue: segmenter + packet assembler
      rc_sq.rs  rc_rq.rs     Phase 3
      retry.rs               Phase 3
    cc/                      Phase 5: ECN, CNP, rate limiter
  examples/                  per-widget traces, per CLAUDE.md §6 Layer C
  doc/                       committed waveform + FSM markdown
  tests/
    vectors/                 scapy-generated reference packets (§10)
```

The Ethernet/IP/UDP layers do **not** live here. They are general-purpose
and belong in `rhdl-fpga`:

```text
crates/rhdl-fpga/src/
  core/crc32.rs              parallel CRC-32, generic over bytes/cycle
  net/                       new category — needs architecture.md sign-off
    mac/                     GMII/RGMII frontend, preamble/SFD, FCS, IFG
    ipv4.rs  udp.rs
```

Adding a top-level category to `rhdl-fpga` is exactly the structural
decision `architecture.md` requires checking before making. `net` is
proposed, not assumed.

### How the pieces compose

```text
   PHY (RGMII, 125 MHz recovered clock)
     |
     |  Circuit (asynchronous) -- the only cross-domain boundary
     v
  net::mac::Rx ---> AsyncFIFO ---> system clock domain
     |                                  |
  net::mac::Tx <--- AsyncFIFO <---------+
                                        |
                        RCStream<Beat<W>, PacketFrame>
                                        |
              +-------------------------+-------------------------+
              |                                                   |
      roce::headers::parse                              roce::headers::generate
      (RX: ACK/NAK/CNP only in Ph.2)                     (TX: BTH + RETH)
              |                                                   |
              v                                                   v
      roce::qp::uc_write_sq  <--- payload RCStream from dsp::ddc etc.
              |
              +--> roce::icrc (runs alongside assembly, see §7)
```

The payload arriving as an `RCStream` is the point. A DDC's output
stream connects to a queue pair the same way it connects to anything
else in the library, with the Carloni relay-station guarantee intact
across the boundary.

### Where `rhdl-rule` earns its place

The send queue is a set of guarded atomic actions over shared state —
"if the segmenter has a beat and the retransmission buffer has room and
the PSN window is open, emit a packet and advance three counters
atomically." Written as a hand-rolled FSM that is a wide `match` with
easy-to-get-wrong interlocks; written as rules it is five independent
`#[rule]` methods and a synthesized conflict-arbitrated scheduler, which
is how blue-rdma expresses the same logic. Phase 3's retry logic is the
stronger case still.

**Decision recorded: the queue-pair logic is written with `rhdl-rule`;
the headers, serialisation and ICRC are plain `#[kernel]` widgets.**
Rules buy atomicity over shared state, which the datapath does not need.

---

## 7 — The ICRC problem

The single most-often-wrong part of a RoCEv2 implementation, and the one
place where a design decision has to be made early.

The Invariant CRC is a CRC-32 (`0x04C11DB7`, the Ethernet polynomial)
computed over **the whole packet from the IP header onward**, but with
every field a router is allowed to mutate replaced by all-ones, and with
an 8-byte all-ones prefix standing in where InfiniBand would have a
Global Routing Header. From Scapy's working implementation, the masking
is:

| Layer | Masked to all-ones |
|---|---|
| prefix | 8 bytes of `0xFF` |
| IPv4 | header checksum, TTL, TOS (DSCP+ECN) |
| IPv6 | hop limit, traffic class, flow label |
| UDP | checksum |
| BTH | `fecn`, `becn`, `resv6` |
| tail | 4-byte `0xFF` ICRC placeholder, excluded from the digest |

The digest covers everything but that final placeholder, is packed
big-endian, and is then **byte-reversed** before being written into the
ICRC field. (Two sources in the literature describe the all-ones prefix
differently — one as a 64-bit dummy LRH, one as 8 bytes replacing the
GRH. They are the same eight bytes; RoCEv2 carries neither header on
the wire. Scapy's code is the tiebreaker and the behaviour to match.)

The engineering consequence, and it is the awkward one: **the ICRC spans
bytes that are generated at different pipeline stages.** The IP and UDP
headers are built by one widget, the BTH by another, the payload arrives
from a stream, and the digest must cover all of them in order — with a
*different* version of the headers than the one that goes on the wire,
because of the masking. BALBOA calls this a "deeply pipelined checksum"
and treats it as a first-class pipeline stage.

Two candidate shapes:

- **(a) Shadow serialiser.** Each header widget emits two byte streams:
  the wire bytes and the ICRC bytes (identical except for masked
  fields). The ICRC engine consumes the second. Costs a second
  serialisation path; wins on clarity, since the masking lives in the
  widget that owns the field and the engine is an ordinary CRC.
- **(b) Masking at the engine.** One byte stream, with a per-beat
  byte-mask side-channel telling the engine which bytes to substitute.
  Costs less logic, but spreads knowledge of the header layout into the
  CRC engine, which is exactly the coupling RHDL's typed headers are
  meant to eliminate.

**Decision deferred to Phase 1, with (a) the presumption.** It should be
decided by measurement — emit both, compare LUT/FF cost and critical
path — and the CHANGELOG entry records which won and why. Writing it
down here so the question is not rediscovered later.

---

## 8 — The variable-length problem: the real language risk

Everything above is engineering. This is the part that might not fit in
RHDL's accepted subset, and it should be confronted in Phase 1 rather
than discovered in Phase 2.

RoCEv2 is fundamentally about **variable-length byte sequences**:

- A message is segmented into PMTU packets; the packet count depends on
  a runtime DMA length.
- The last packet is ragged, with `padcount` of 0–3 pad bytes.
- Headers are 12, 16, 28 or 32 bytes depending on the opcode, so at any
  datapath width above 8 bits the payload's first byte lands at a
  runtime-dependent offset in a beat.
- On a wide datapath, a header *straddles* beats, and extraction needs a
  barrel shift by a runtime amount.

RHDL's kernel subset allows `for i in 0..N` with constant `N` only, and
CLAUDE.md §4 is explicit that there is no `while`, no `loop`, and no
runtime-bounded iteration. The known-good pattern is to unroll over the
datapath width with a runtime *index* selecting among the unrolled
results — and runtime indexing has already bitten this project: the
CORDIC work hit it with `ATAN_TABLE[k]`, and the DDC notes flag it as
likely to bite a CIC's comb delay line.

**The mitigation is architectural, and it is the standard one:** at 1 GbE
with an 8-bit datapath, *every beat is one byte*, so there is no
intra-beat alignment problem at all. Header fields land on beat
boundaries, the ragged tail is a beat count, and segmentation is a
counter. The variable-length problem reduces to counters and a state
machine, which RHDL does well.

That is the third independent reason to start at 1 GbE. It also means
the width-generic version is a real research question rather than a
refactor, and §11 records it as such.

**Phase 1 exit criterion: a byte-serial header serialiser and parser
that compiles, and a written answer to whether the width-generic version
is expressible.** If the answer is no, that is a
`kernel-language-extensions.md` input — probably the strongest
motivating case that document will ever get — and it is better to learn
it from a 4-week Phase 1 than from a stalled Phase 2.

---

## 9 — Phased implementation

Each phase is a coherent deliverable that works end to end, per
CLAUDE.md §11.1's "one feature per PR" sized to the ask. Estimates
assume one engineer and include the full five-tier validation,
examples, traces and CHANGELOG entries that every widget in this
repository carries — which is roughly 3× the raw-RTL effort and is not
optional.

### Phase 0 — The dependencies (8–10 weeks)

Nothing RoCEv2-specific. All three are already wanted by
`widget-roadmap.md` and are useful whether or not this project continues.

1. **`core::crc32`** — parallel CRC-32, generic over bytes-per-cycle,
   with byte-enable for a ragged final beat, reflection and final
   XOR-out. Validated against the standard CRC-32 check value and
   against `core::crc`'s bit-serial engine on random messages. *(~2 wk)*
2. **`net::mac`** — roadmap item 59. GMII/RGMII frontend, preamble and
   SFD, FCS append/check, inter-frame gap, the recovered-clock CDC. The
   first genuinely asynchronous `Circuit`-family widget of real size.
   Validated by loopback against itself and by replaying captured
   frames. *(~5 wk)*
3. **`net::ipv4` + `net::udp`** — header construction and parsing,
   IPv4 header checksum, and the masked re-serialisation §7 needs.
   *(~2 wk)*

*Gate: a frame generated by RHDL is accepted by a real NIC and shows up
in `tcpdump`.* Until that is true, nothing above it can be debugged.

### Phase 1 — Packet level, stateless (4–6 weeks)

BTH/RETH/AETH/ImmDt as `Digital` types with the opcode enums of §2;
byte-serial parse and generate; the ICRC engine and the §7 decision.
No queue pair, no state, no memory.

*Gate: RHDL parses every packet Scapy generates, and Scapy validates the
ICRC of every packet RHDL generates, over a vector set covering all
implemented opcodes. Plus the §8 write-up.*

This phase is where the project is cheapest to cancel, and it produces
the academic-paper-level artifact (a verified parser/generator) on its
own.

### Phase 2 — UC RDMA WRITE, transmit (6–8 weeks)

Queue-pair context table; message segmenter over PMTU; PSN counter;
packet assembler; out-of-band connection setup (a UDP connection
manager, as FERoCE does); the minimal receive path needed to consume
and ignore what UC does not use.

*Gate: an RHDL endpoint writes into a buffer registered by an unmodified
Linux host, verified by the host reading its own memory. §10 Tier 7.*

**This is the deliverable the use case needs.** 18–24 weeks cumulative.

### Phase 3 — Reliable Connection (16–24 weeks)

ACK/NAK generation and consumption, PSN window tracking, the
retransmission buffer and its timers, duplicate detection, RNR-NAK, and
the receive-queue request handler. This is the third of the problem that
§3's file sizes measure. `rhdl-rule` carries the retry logic.

*Gate: `ib_write_bw` from `perftest` completes against the RHDL endpoint
with zero retransmission errors, and completes again with deliberately
injected loss.*

### Phase 4 — READ, SEND/RECV, completions, memory regions (12–16 weeks)

An RDMA READ responder needs local memory reads and therefore R_Key
validation and address translation — the protection domain machinery
Phases 2–3 avoid entirely. Work-completion generation makes the endpoint
usable from ordinary verbs code.

### Phase 5 — Congestion control (8–12 weeks)

ECN marking, CNP generation and consumption, and the DCQCN rate-limiter
control loop. Only needed on a shared fabric; on a point-to-point link
it is dead logic. Sequence it when a real deployment needs it, and note
that the rate controller is a DSP control loop, which this repository is
now well equipped to build and test.

### Totals

| Target | Cumulative |
|---|---|
| Phase 0 — a frame on the wire | 8–10 weeks |
| Phase 1 — verified packet parser/generator | 12–16 weeks |
| **Phase 2 — UC WRITE endpoint, data in host memory** | **18–24 weeks (4.5–6 months)** |
| Phase 3 — Reliable Connection | 34–48 weeks |
| Phase 4 — full verb set | 46–64 weeks |
| Phase 5 — congestion control | 54–76 weeks (13–18 months) |

The 13–18 months for the full stack is consistent with blue-rdma's
26 kLOC and with `tier-c-flagship-cores.md`'s 18–24 months for
comparable projects.

---

## 10 — Validation

CLAUDE.md §5's five tiers apply per widget and are not restated. A
network stack needs two more, because `iverilog` agreeing with the Rust
simulator proves only that RHDL is self-consistent — it says nothing
about whether a NIC will accept the packet.

### Tier 6 — Vector interoperability (cheap, high value, starts in Phase 1)

Scapy's `contrib/roce.py` generates reference packets and validates
ICRCs. Committed vectors under `tests/vectors/`, generated by a script
that is committed alongside them, with the generating Scapy version
pinned.

- **Parse direction:** for every vector, the RHDL parser's `Digital`
  output equals the expected field values.
- **Generate direction:** for every vector, RHDL's bytes equal Scapy's
  bytes, ICRC included.

Determinism: the vectors are committed artifacts, so CLAUDE.md §8's
clean-tree rule applies — the generator script is run deliberately, and
the test only checks.

### Tier 7 — Against a real software RDMA stack (Phase 2 gate)

Linux ships `rxe` (Soft-RoCE), a software RoCEv2 implementation. It
speaks the real protocol and runs on any machine with no special
hardware.

- **RHDL → rxe:** the simulation emits a pcap; it is replayed onto a tap
  interface that `rxe` is bound to; the host application reads its own
  registered buffer and finds the expected bytes.
- **rxe → RHDL:** `tcpdump` captures what `rxe` emits for the same
  operation; the capture is replayed into the RHDL testbench as stimulus.

This harness is the highest-value thing in this document after the ICRC
decision, because it converts "we believe this is RoCEv2" into "an
independent implementation agrees." It should be built in Phase 1, used
in anger in Phase 2, and is the acceptance gate for every phase after.

### Tier 8 — On hardware against a commodity NIC (Phase 2 exit, hardware-gated)

An FPGA board whose PL owns a MAC, on a direct link to a host with any
ConnectX-class NIC or just a plain NIC running `rxe`. Bandwidth and
latency measured with `perftest`. Blocked until §4's board question is
answered.

---

## 11 — Risks and open questions

1. **The width-generic datapath may not be expressible** in today's
   kernel subset (§8). Mitigated by starting byte-serial; the Phase 1
   write-up is the decision point, and the answer is an input to
   `kernel-language-extensions.md` either way.
2. **No board currently owns a MAC in the PL** (§4). Phase 0 can be
   developed and simulated without one, but Tier 8 cannot happen and the
   project should not reach Phase 2 without this answered.
3. **UC is only safe on the topology we have.** If the instrument ever
   lands on a shared, congested fabric, Phase 3 stops being optional and
   becomes a correctness requirement, not a performance one. Recorded
   here so it is not a surprise.
4. **The retransmission buffer is a bandwidth-delay product.** At 1 GbE
   it is block RAM; at 10G and above it is external memory and the
   project acquires a memory-controller dependency it does not have
   today.
5. **`rhdl-rule` has not been used at this scale.** Three pilot widgets
   is not a 26 kLOC core. Phase 2's send queue is the first real test,
   and discovering a rule-scheduler limitation there is a plausible
   outcome — which is a reason to do it, but the schedule should hold
   slack for it.
6. **Copyleft proximity.** The best references are GPL-family. The
   discipline is: read for architecture, use as oracles, write our own
   from the IBTA specification, and keep `roce-subset-spec.md` as the
   evidence of independent derivation.
7. **An unoptimised 1 GbE datapath is 8 bits at 125 MHz.** That should
   close comfortably, but the ICRC and FCS paths are the long ones, and
   `auto-pipelining-plan.md` has not shipped. Expect hand-placed relay
   stations and note where they are.

---

## 12 — What is being asked for

This document commits nothing. Before Phase 0 begins, three decisions
are needed:

1. **Does this project get scheduled at all,** given it is
   Tier-C-scale and competes with RV32I, the Alto and the Tier 0–4
   widget library?
2. **Is the §9 staging agreed as staging?** CLAUDE.md's anti-sliver rule
   requires explicit sign-off on splitting work this size across PRs,
   and Phase 2 shipping as "UC RDMA WRITE transmit at 1 GbE" rather than
   as "RoCEv2" is part of what is being agreed.
3. **Is a `net` category in `rhdl-fpga` acceptable** (§6), or should the
   Ethernet/IP/UDP layers live elsewhere? This is an `architecture.md`
   question and is the one structural decision Phase 0 cannot start
   without.

Phase 0 is independently worthwhile: a parallel CRC-32 and an Ethernet
MAC are both on the roadmap on their own merits, and the MAC is
described there as "the killer demo." If the answer to (1) is no, Phase 0
is still worth doing.
