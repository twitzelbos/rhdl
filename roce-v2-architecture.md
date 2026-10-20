# RoCEv2 — RDMA over Converged Ethernet v2 for RHDL

**Status: scope document. No code written. The staging in §9 needs explicit
sign-off before Phase 0 starts** — see §12.

RoCEv2 lets a network card write directly into a remote machine's memory
with no CPU in the data path — and on a host with `nvidia-peermem`
loaded, directly into **GPU memory**, bypassing host RAM entirely. The
target application is an instrument that deposits acquired samples
straight into a GPU buffer for processing, with no driver, no socket, no
copy and no interrupt per transfer.

**The sender does not have to be an RDMA NIC.** That is the single most
important fact in this document and §4.3 is built on it: a RoCEv2 RDMA
WRITE is an ordinary UDP datagram, the *receiving* NIC's engine does the
placement, and the sender only has to emit correctly-formed packets. This
is why the architecture works on a Red Pitaya, whose Ethernet is wired to
the processing system rather than to the fabric.

Three things are in scope from the outset, in this order: **RDMA WRITE
send**, then **RDMA WRITE receive**, and throughout, **independence from
whatever carries the frames** — the hard PS GEM and an own MAC are
interchangeable adapters on one boundary (§5.1), not two versions of the
stack.

It is a plan, not a proposal to start tomorrow. Samples landing in host
GPU memory is **3–4.5 months**; adding the receive direction takes it to
**4.5–6 months**; a full Reliable Connection stack with every verb is
**14–20 months for one engineer**.

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

Second, there is a measurement in §3 that makes the case better than
any argument could. The deliberately-minimal, WRITE-only reference
implementation is **1.05 MB of SystemVerilog**; the *full* Reliable
Connection implementation is **909 KB of Bluespec**. A far smaller
feature set costs more code in the lower-level language. RHDL now ships
`rhdl-rule` — Bluespec-style guarded atomic rules, 7.6 kLOC across three
crates with ~20 integration tests and three pilot widgets — so a RoCEv2
queue pair is both the canonical workload for that extension and a
direct, publishable test of whether the saving reproduces.

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

## 3 — Reference implementations, compared

The samples on GitHub are unusually good, and they do not all deserve
the same weight. Sizes below are `git` blob bytes from each repository's
tree — a crude proxy for complexity, but a consistent one.

| Project | Language | Scope actually implemented | Core size | Rate, measured on |
|---|---|---|---|---|
| **FERoCE** / [Lightweight-RoCEv2-Stack](https://github.com/CodeByRachit/Lightweight-RoCEv2-Stack) | SystemVerilog | RDMA WRITE + WRITE_WITH_IMM, **TX only**; RX decodes ACKs only; retransmission; UDP connection manager | **39 files, 1.05 MB** (own RTL, excluding vendored libs) | 10G and 25G on VCU118 |
| [blue-rdma](https://github.com/datenlord/blue-rdma) | Bluespec SV | **Full RC**: send + receive queues, retry, duplicate-read cache, work completions, atomics | **22 files, 909 KB** | not stated |
| [Coyote](https://github.com/fpgasystems/Coyote) / RoCE BALBOA | Vitis HLS + SV | WRITE + READ, retransmission, DCQCN | HLS core ~11 KB + ~450 KB SV plumbing, **of which `icrc.sv` alone is 304 KB** | 100G, Alveo + HBM |
| [RoCKS](https://github.com/ETH-PBL/RoCKS) | packaging | *Is* Coyote's stack — byte-identical 305 KB `icrc.sv` — plus a Linux driver and a Yocto layer | — | 10/25G over KR260's SFP+ |
| [scapy `contrib/roce.py`](https://github.com/secdev/scapy/blob/master/scapy/contrib/roce.py) | Python | headers + ICRC | — | test oracle |

### FERoCE is the primary reference — not blue-rdma

The first draft of this document had that the wrong way round. blue-rdma
is the most *complete* implementation, which made it look like the one to
follow; but completeness is not what we need a reference for. **FERoCE's
scope is exactly the Phase-2 target**, in its authors' own words:

> RoCEv2 is a complex protocol, but not all its features are required
> for this project. [...] The goal is only to push data and initiate the
> RDMA transfer, for this reason only RDMA WRITE is considered.

It supports WRITE and WRITE_WITH_IMMEDIATE; an FSM splits the input
stream into ONLY/FIRST/MIDDLE/LAST by PMTU; the RX path decodes only ACK
packets; QP parameters are set in FPGA registers from a UDP
connection-manager exchange. That is Phase 2 of §9, already built and
already measured against a commodity NIC. It is also *motivated by the
same problem we have* — it is a CERN DAQ project, and its opening
premise is that a large fraction of CPU is consumed by moving acquired
data.

Its module decomposition is therefore the one to study, and it maps
cleanly onto §6:

| FERoCE module | Size | RHDL counterpart |
|---|---|---|
| `RoCE_tx_engine.sv` | 73 KB | `qp::uc_write_sq` |
| `RoCE_rtr_read_module.sv` + `RoCE_retransmission_module_v2.sv` | 122 KB | Phase 3 `qp::retry` |
| `RoCE_tx_header_producer.sv` | 45 KB | `headers::generate` |
| `RoCE_reth_bth_tx.sv`, `RoCE_immdh_reth_tx.sv`, `RoCE_bth_udp_tx.sv` | 80 KB | `headers::{bth,reth}` + `serde` |
| `RoCE_realign_frame_fifo.sv` | 35 KB | **§8's alignment problem, as a module** |
| `axis_RoCE_icrc_insert.v` | 28 KB | `icrc.rs` |
| `RoCE_qp_state_module.sv` | 28 KB | `qp::context` |
| `udp_RoCE_connection_manager*.sv` | 72 KB | Phase 2 connection setup |
| `RoCE_bth_udp_rx.sv` | 27 KB | `headers::parse` (ACK path) |

### The measurement that argues for `rhdl-rule`

Put the first two rows of the table together:

- **FERoCE** — WRITE-only, transmit-only, one transport — is
  **1.05 MB of SystemVerilog across 39 files**.
- **blue-rdma** — full Reliable Connection, both directions, retry,
  duplicate-read cache, work completions, atomics — is **909 KB of
  Bluespec across 22 files**.

A **much smaller feature set costs more code in SystemVerilog than a
much larger feature set costs in Bluespec.** That is the strongest
argument in this document for writing the queue-pair logic in
`rhdl-rule`, and it is a measurement rather than an opinion. blue-rdma's
role here is to be that comparison, and to be the Phase-3 reference once
reliable transport is on the table — its
`ReqHandleRQ` + `RespHandleSQ` + `RetryHandleSQ` are ~262 KB, nearly a
third of it, which is what sets Phase 3 at 16–24 weeks.

### Nobody writes their own parallel CRC-32

FERoCE's `lib/` has exactly three vendored dependencies, and **two of
the three are CRC**: `FAST_CRC32`, `Tbps_CRC`, and `eth`. Its own slides
say it plainly — for 10/25G, CRC-32 must be computed over 64-bit words
at 156.25 or 390.625 MHz, and *"new module for 100G, parallel
computation is needed here. CRC computed for 512b data words at 322.266
MHz, not that easy."*

Coyote agrees by construction: `icrc.sv` at **304 KB is the single
largest RTL file in its entire RDMA directory**, dwarfing
`roce_stack.sv` at 21 KB. At a 512-bit datapath a parallel CRC-32 is a
vast unrolled XOR tree, and it is generated, not written.

This is the clearest possible confirmation of §4's gap 2 and of §7: the
ICRC is the hard part of a RoCEv2 implementation, everyone treats it as a
separate engineering artifact, and RHDL's bit-serial `core::crc` is not a
starting point for it.

### The third dependency is MIT, which changes the licensing picture

`eth` is [Alex Forencich's **verilog-ethernet**](https://github.com/alexforencich/verilog-ethernet)
— **MIT licensed**, 3.1k stars, and the de-facto standard open Ethernet
MAC + ARP + IP + UDP stack for FPGAs. Both FERoCE and RoCKS build on it.
It ships validated example designs for GMII, RGMII *and* SGMII
(`KC705/fpga_{gmii,rgmii,sgmii}`), which is precisely the Phase-0
`net::mac` surface.

So the blanket "all the references are copyleft" in the first draft was
wrong, and the distinction matters:

- **Phase 0 (MAC, IP, UDP) has a permissively-licensed reference** we may
  read freely and model our architecture on.
- **Phase 1+ (the RoCE layer) does not.** blue-rdma, RoCKS and Scapy are
  GPL-family; Coyote is its own licence. For those we read for
  architecture, use as oracles, and write our own from the IBTA
  specification, keeping `roce-subset-spec.md` as the evidence of
  independent derivation.

### No reference implementation runs at 1 GbE

Worth stating, because it cuts against §5. Every one of these runs at
10G or above: FERoCE on a VCU118 at 10/25G, RoCKS over KR260's 10G SFP+,
BALBOA at 100G on Alveo. FERoCE lists *"scalable 1/10/100 Gb/s"* as a
requirement and implemented 10 and 25.

A 1 GbE RoCEv2 endpoint is therefore **unprecedented among the
references**. That is simultaneously why it is tractable for us (§8) and
a risk: the byte-serial ICRC and the 8-bit segmentation path would be
ours alone, with no prior art to check against. §10's interoperability
tiers are the mitigation, and Soft-RoCE works happily over 1 GbE.

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

### 4.3 — Hardware: the sender does not need to be an RNIC

**A Red Pitaya can do this.** An earlier draft of this document said it
could not, and that was wrong — it mis-sited where the offload has to
live.

The reasoning that led to the error was: the PL cannot reach the Ethernet
PHY, therefore the CPU is in the data path, therefore RDMA is pointless.
The first clause is true and the third does not follow. **The RDMA
offload that matters is on the *receiving* side.** A RoCEv2 RDMA WRITE is
an ordinary UDP datagram to port 4791 with a structured payload; the host
NIC's RDMA engine is what parses it and places the data, and on a host
with `nvidia-peermem` loaded it places that data **directly into GPU
memory** with no host CPU and no staging through host RAM. The sender
only has to emit correctly-formed packets. It does not need a queue pair
in hardware, does not need to be an RNIC, and does not need to be an
FPGA at all.

This is established practice, not a workaround. CERN's FERoCE has the
server create the QP and send the QP parameters to the FPGA over UDP —
the FPGA is a sender-only endpoint by design. The CTAO-LST camera readout
does the same thing at 10 GbE, validated against a PC running Soft-RoCE.

### What the Red Pitaya architecture looks like

```text
  PL                                          |  PS              |  wire
  ----------------------------------------    |  --------------  |  ------
  ADC -> dsp::ddc -> RCStream<Iq,_>           |                  |
            |                                 |                  |
            v                                 |                  |
   roce::qp::uc_write_sq   (segment by PMTU,  |                  |
            |              PSN counter)       |                  |
            v                                 |                  |
   roce::headers::generate (Eth+IP+UDP+BTH    |                  |
            |              +RETH)             |                  |
            v                                 |                  |
   roce::icrc  (byte-wide CRC-32)             |                  |
            |                                 |                  |
            v                                 |                  |
   complete frame -> AXI-HP -> DDR ring buffer |                 |
                                   |          |                  |
                                   +--------> | post GEM         |
                                              | descriptor  -----+--> GEM
                                              | (no protocol     |    adds FCS
                                              |  work at all)    |    sends
```

The PL builds the **entire frame including the ICRC**. The PS does no
protocol work whatsoever — it points a GEM scatter-gather descriptor at a
buffer the PL has already filled and clears the used bit. GEM appends the
Ethernet FCS in hardware and transmits. The CPU cost per packet is a
couple of memory writes, not a protocol stack.

### This makes the project substantially smaller

Five of the hardest items in the original plan disappear, because the PS
GEM *is* the MAC:

| Originally needed | Status on this architecture |
|---|---|
| `net::mac` — GMII/RGMII, preamble/SFD, IFG, recovered-clock CDC | **Not needed.** GEM is the MAC. This was the 5-week Phase-0 item. |
| Ethernet FCS generation | **Not needed.** GEM does it in hardware. |
| ARP | **Not needed.** The host's MAC is a connection-setup parameter. |
| Parallel CRC-32 at 512 bits / 322 MHz | **Byte-wide at 125 MHz suffices** — 8 bits/cycle *is* 1 Gbps. A modest XOR tree, not a generated monster. |
| ACK path, retransmission buffer, PSN window | **Not needed for UC** — a monotonic PSN counter is the whole of it. |

What remains is the part RHDL is actually good at and that no amount of
board selection removes: the typed headers, the ICRC, the PMTU
segmenter, and the queue-pair context.

### The constraint that actually bites: packets per second

The PL side is easy. The real question is how fast the PS can retire
descriptors, and there is a hard limit in the way.

**The Zynq-7000 PS GEM supports a maximum frame size of 1522 bytes** —
jumbo frames are a PL-side AXI Ethernet feature, not a PS GEM one. With
58 bytes of Eth+IP+UDP+BTH headers plus a 4-byte ICRC, that caps the
RoCEv2 PMTU at **1024** (2048 would need a 2106-byte frame). So at full
line rate:

```text
  frame on wire = 1024 payload + 58 hdr + 4 ICRC + 4 FCS + 20 preamble/IFG
                = 1110 bytes
  1 Gbit/s / 1110 bytes  ~=  113,000 packets/s
```

113k descriptors/s on a 667 MHz Cortex-A9 is the open question. For
calibration, Express Logic measured 910–940 Mbps of *TCP* on a Zynq-7000
using one A9 core — but with checksum offload and a PL-side DMA engine,
and at 1500-byte frames (~80k pps). Our per-packet work is far cheaper
than a TCP stack, so it is plausible; it is not proven.

**Mitigations, in order of preference:** a pre-populated descriptor ring
the PL fills and the PS only advances; batching; a dedicated core;
`AF_XDP` if going through the kernel at all. And the honest one — see
below — most likely not needing line rate.

**This is measurable today, before any HDL exists.** Blast pre-built
1086-byte frames from a userspace loop on the Red Pitaya through
`AF_PACKET`/`AF_XDP` and count what the PS retires. That experiment costs
an afternoon and is the single highest-value thing to do before
committing to this plan — it is the one result that could invalidate the
architecture, and it needs no RHDL at all. **It should be Phase −1.**

### And the bandwidth reality

1 GbE is 125 MB/s. That bounds what this can carry:

- **Post-DDC data: comfortable, and the pps problem evaporates.** The
  whole point of the decimation chain is 125 MSPS down to kHz-range
  bandwidth. At, say, 5 MB/s you need ~5k packets/s, which the PS will
  retire without noticing. **For the NMR use case as it stands, the
  throughput question is moot** — which is also why the value here is
  latency, determinism and host-CPU freedom rather than bandwidth.
- **Raw 2-channel 14-bit at 125 MSPS is 437 MB/s** — 3.5× over 1 GbE,
  and no protocol choice changes that. If the raw stream is the
  requirement, this needs a different board, and §4.4's list applies.

### 4.4 — If and when 1 GbE is not enough

Then the original analysis applies and the requirement becomes a board
whose PL owns the Ethernet interface. `verilog-ethernet`'s validated
examples are the useful shortlist, since they are the boards on which the
MAC we would model `net::mac` on is known to work:

| Rate | Board | Device | Notes |
|---|---|---|---|
| 1 GbE | **Nexys Video** | Artix-7 200T | RGMII PHY on PL. |
| 1 GbE | **KC705** | Kintex-7 325T | GMII, RGMII *and* SGMII examples — the best MAC reference. |
| 1 GbE | ZCU102 / ZCU106 | Zynq US+ | PL-side Ethernet plus a PS for the control plane. |
| 10G | **KR260** | Zynq US+ K26 | 4× RJ45 1G + 1× SFP+ 10G. RoCKS's board — cheapest hardware that already runs a reference stack. |
| 10/25G | VCU118 | Virtex US+ VU9P | FERoCE's board. |
| 100G | Alveo U50/U55/U280 | | BALBOA's class; needs HBM for retransmission. |

Note what does *not* change: the RoCE layer is the same widgets. Only
`net::mac` gets added underneath and the datapath widens, which is
exactly where §8's alignment problem arrives.

### 4.5 — What the host side needs

This is a procurement constraint, and it is not negotiable by anything
on the FPGA:

- **An RDMA-capable NIC** — ConnectX-3 VPI or newer for
  `nvidia-peermem`. RoCE is explicitly supported, not just InfiniBand.
- **`nvidia-peermem` loaded**, plus kernel RDMA peer-memory support
  (via MLNX_OFED or a patched kernel, or the newer DMA-BUF path). It
  does not auto-load; `modprobe nvidia-peermem`. If the NVIDIA driver
  was installed before MLNX_OFED it must be reinstalled so the module
  compiles against MOFED's RDMA API.
- **NIC and GPU on the same PCIe root complex** for best performance.
- **A GPU that NVIDIA supports for GPUDirect RDMA.** The support matrix
  is datacenter/professional parts; **consumer GeForce is not officially
  supported** and should be verified before being designed around. This
  is the most likely procurement surprise in the whole project.

### 4.6 — The alternative that needs no protocol work at all

Worth knowing before committing, because it moves the whole problem to
the host: **NVIDIA DOCA GPUNetIO** (and its predecessor, DPDK's
`gpudev`) receives **plain Ethernet/UDP** packets directly into GPU
memory. It uses GPUDirect RDMA underneath — `nvidia-peermem` is still
required — but the *wire protocol is ordinary UDP*.

| | RoCEv2 | DOCA GPUNetIO |
|---|---|---|
| FPGA work | headers, ICRC, PMTU segmenter, QP context | **none** — send plain UDP |
| Host work | none in the data path; the NIC places the data | a poll-mode driver on a dedicated core; reassembly in a CUDA kernel |
| Host hardware | RDMA NIC + peermem + supported GPU | NVIDIA NIC (ConnectX/BlueField) + peermem + supported GPU |
| Ordering/loss handling | the protocol's, for free | yours to write |
| Determinism | NIC hardware places it | depends on the poll loop |

If the goal is purely "samples in GPU memory, soon," GPUNetIO gets there
with no FPGA protocol work and should be the baseline that any RoCEv2
effort is measured against. RoCEv2 wins on host CPU (truly zero in the
data path), on not owning the reassembly logic, and on the §1 strategic
grounds. **It is an honest comparison that should be made explicitly
rather than assumed away**, and it belongs in §12's decisions.

## 5 — Scope decision: send first, then receive, link-agnostic throughout

Three decisions recorded, in priority order.

### 5.1 — The RoCE core is independent of the Ethernet interface

**Decision recorded: the RoCE layer presents a framed byte-stream
boundary in both directions and knows nothing about what carries it.**
The hard GEM path and an own-MAC path are two interchangeable *adapter
widgets* on that boundary, not two versions of the stack.

This is a requirement from the outset, not a later generalisation, and it
is the one that most shapes the code. It is also cheap to honour if
designed in and expensive to retrofit, because an implementation that
assumes a MAC underneath tends to leak frame timing, FCS handling and
backpressure semantics into the header and segmenter logic.

The boundary type is `RCStream<Beat<W>, FrameMark>` — the same bus
everything else in the library speaks, which means the core can be tested
end-to-end with no link at all, driven from a vector file (§10 Tier 6).
`W` is the beat width: 8 for the GEM path and for GMII at 1 GbE, 64 at
10G, with §8's alignment caveat applying only above 8.

**The adapter is chosen by composition, not by a type parameter.** No
RoCE widget carries a `Link` generic. This follows the decision already
recorded in `vendor-primitive-architecture.md` — widgets are written once
and do not thread a target generic through their API; the choice is made
at the composition site. Here the composition site instantiates
`roce::link::GemRing` or `roce::link::MacPhy` as a sibling widget and
wires it to the core's framed-stream ports.

### 5.2 — Send before receive

**Decision recorded: RDMA WRITE *send* is built first; RDMA WRITE
*receive* second.** Both are in scope and neither is optional; the order
is send-first because:

1. **Send is what the sensor use case needs**, and it is the direction
   that delivers a working system end to end — samples arriving in host
   GPU memory.
2. **Send is the clean direction on the GEM path and receive is not**
   (see §5.4). Building send first means the architecture's weakest seam
   is discovered against a working baseline rather than at the start.
3. **Receive reuses everything send builds** — the same headers, the
   same ICRC engine (checking rather than generating), the same QP
   context — plus a memory-region table and local placement. It is an
   extension, not a parallel effort.

### 5.3 — UC before RC

**Decision recorded: the Unreliable Connection transport comes before
Reliable Connection**, in both directions. RC, READ, SEND/RECV, atomics
and congestion control are later phases, explicitly staged, not silently
dropped.

CLAUDE.md's anti-sliver rule requires justifying this rather than
asserting it, because "ship the easy transport first" is exactly the
shape that rule forbids:

1. **UC WRITE is a complete, externally-verifiable deliverable.** It
   interoperates with an unmodified Linux RDMA stack; the acceptance test
   is a host application reading its own GPU buffer. That is a whole
   thing that works, which is what the anti-sliver rule actually asks.
2. **UC is not a degraded RC on this topology.** On a direct or
   single-switch instrument link there is nothing to drop frames; UC's
   lack of acknowledgement costs nothing the fabric was going to provide.
   And UC's failure mode suits streaming sensor data exactly — a PSN gap
   loses the remainder of one message and resynchronises on the next
   FIRST/ONLY, rather than tearing down the connection.
3. **It defers precisely the third of the problem that §3's file sizes
   identify as the expensive third**, and nothing else.
4. **The precedent exists**: FERoCE is a deployed, published WRITE-only
   endpoint; CTAO-LST ships the same shape at 10 GbE.

What this does *not* license: calling any phase "RoCEv2 support" without
qualification, or opening the RC phase before the UC phases are
interoperable against real hardware. Each CHANGELOG entry names the
transport and the direction in its title.

### 5.4 — The honest asymmetry between the two directions on GEM

Worth recording because it is the strongest technical argument for why
§5.1's abstraction has to exist:

- **Sending through the PS GEM is clean.** The PL builds complete frames
  autonomously into a DDR ring; the PS only advances descriptors. The CPU
  is not in the data path in any meaningful sense.
- **Receiving through the PS GEM is not.** GEM DMAs an arriving frame
  into DDR and the PS must then be notified and hand it to the PL. The
  CPU *is* in the receive data path, per packet, with the latency and
  jitter that implies.

For a sensor this is tolerable, because the receive direction carries
control, configuration and — once RC lands — acknowledgements, all of
which are low-rate. **But high-rate RDMA receive is the case that
justifies the own-MAC path**, and that is exactly why the link boundary
is a first-class design decision rather than a portability nicety. The
same core, a different adapter, no rewrite.

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
    icrc.rs                  the masked-field CRC32 (see §7), gen + check
    qp/
      context.rs             queue-pair context table (RAM-backed)
      uc_write_sq.rs         UC send queue: segmenter + packet assembler
      uc_write_rq.rs         UC receive: place payload at RETH address
      mr.rs                  memory-region table; rkey check + translation
      rc_sq.rs  rc_rq.rs     RC phase
      retry.rs               RC phase
    link/                    §5.1 -- the interchangeable interface adapters
      mod.rs                 the RCStream<Beat<W>, FrameMark> boundary
      gem_ring.rs            AXI-HP master <-> DDR ring the PS GEM serves
      mac_phy.rs             net::mac::{Tx,Rx} on our own MAC
    cc/                      congestion-control phase: ECN, CNP, rate limiter
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

On the Red Pitaya target (§4.3) there is no MAC and no PHY domain — the
pipeline ends at a DDR ring buffer:

```text
   payload RCStream (from dsp::ddc, &c.)
            |
            v
   roce::qp::uc_write_sq      segment by PMTU, advance PSN
            |
            v
   roce::headers::generate    Eth + IP + UDP + BTH + RETH
            |
            +--> roce::icrc   byte-wide CRC-32, see §7
            |
            v
   roce::tx::ddr_ring         AXI-HP master; complete frames
                              into a buffer ring the PS owns
```

**Everything above the dashed line is the same widgets in both cases.**
On a board whose PL owns the MAC (§4.4) only the adapter changes, and
only then does the asynchronous PHY domain and its CDC appear:

```text
   PHY (RGMII, recovered clock) -- Circuit family, the only cross-domain edge
     |
  net::mac::{Rx,Tx} <--> AsyncFIFO <--> system clock domain
                                             |
                               roce::headers::{parse,generate}
                                             |
                                   roce::qp::uc_write_sq
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

### Phase −1 — Measure the send path (one afternoon, no RHDL)

Blast pre-built 1086-byte frames from userspace on the Red Pitaya through
`AF_PACKET`/`AF_XDP` and count what the PS actually retires. §4.3 needs
~113k packets/s for line rate and far less for the real data rate.

*Gate: a number.* This is the only experiment that can invalidate the
whole architecture, it costs nothing, and it requires no RHDL. **Do it
first.** If the PS cannot retire enough descriptors, the answer is a
board from §4.4 and Phase 0 grows the MAC back.

### Phase 0 — The dependencies (3–4 weeks on the Red Pitaya target)

1. **`core::crc32`** — CRC-32 generic over bytes-per-cycle, with
   byte-enable for a ragged final beat, reflection and final XOR-out.
   Byte-wide is enough here (8 bits/cycle at 125 MHz *is* 1 Gbps), but
   the widget should be width-generic for §4.4's sake. Validated against
   the standard check value and against `core::crc`'s bit-serial engine
   on random messages. On the roadmap already. *(~2 wk)*
2. **`net::ipv4` + `net::udp`** — header construction, the IPv4 header
   checksum, and the masked re-serialisation §7 needs. *(~2 wk)*

**`net::mac` is not in Phase 0 on this architecture** — the PS GEM is the
MAC. It returns only if §4.4 applies, and it stays on the roadmap as item
59 on its own merits.

*Gate: a frame built by RHDL, handed to GEM, shows up correctly in
`tcpdump` on the host.*

### Phase 1 — Packet level and the link boundary (4–6 weeks)

BTH/RETH/AETH/ImmDt as `Digital` types with the opcode enums of §2;
byte-serial parse **and** generate; the ICRC engine in both directions
(generate and check) and the §7 decision; and the §5.1
`RCStream<Beat<W>, FrameMark>` boundary defined, with a vector-file
driver standing in for a link. No queue pair, no state, no memory.

Parse and check are built here rather than deferred to the receive phase
because they cost almost nothing alongside generate — the header types
are shared — and because the Tier-6 oracle needs both directions to be
useful at all.

*Gate: RHDL parses every packet Scapy generates, and Scapy validates the
ICRC of every packet RHDL generates, over a vector set covering all
implemented opcodes. Plus the §8 write-up.*

This phase is where the project is cheapest to cancel, and it produces
the academic-paper-level artifact — a verified parser/generator — on its
own.

### Phase 2 — UC RDMA WRITE, send, over the GEM adapter (6–8 weeks)

The first deliverable that moves data. Queue-pair context table; message
segmenter over PMTU; PSN counter; packet assembler; `link::gem_ring`
(AXI-HP master into a DDR ring) plus the thin PS-side descriptor poster;
out-of-band connection setup over UDP, as FERoCE does.

*Gate: an RHDL endpoint writes into a buffer registered by an unmodified
Linux host, verified by the host reading its own memory — and then, with
`nvidia-peermem` loaded and the buffer allocated with `cudaMalloc`,
verified by a CUDA kernel reading its own GPU memory. §10 Tier 7.*

**This is the deliverable the sensor use case needs.** 13–18 weeks
cumulative, on existing Red Pitaya hardware.

### Phase 3 — UC RDMA WRITE, receive (6–8 weeks)

The other direction, per §5.2. Reuses Phase 1's parse and ICRC check and
Phase 2's QP context, and adds what receiving requires: `qp::mr` — the
memory-region table, R_Key validation and address translation — and the
local placement path that writes an incoming payload to the address the
RETH names. Still UC, so still no acknowledgements.

Note that on the GEM adapter this direction has the CPU in the data path
per §5.4, so the acceptance target is correctness and modest rate, not
line rate. Line-rate receive is what Phase 4 unlocks.

*Gate: an unmodified Linux host issues `ibv_post_send` of an RDMA WRITE
to the RHDL endpoint and the payload appears at the expected local
address, with a deliberately corrupted ICRC rejected.*

### Phase 4 — The own-MAC adapter (5–7 weeks, hardware-gated)

`net::mac` — roadmap item 59 — plus `link::mac_phy`. GMII/RGMII
frontend, preamble and SFD, FCS append and check, inter-frame gap, and
the recovered-clock CDC. The first genuinely asynchronous
`Circuit`-family widget of real size.

This phase is the **proof of §5.1**: if the RoCE core needs any change to
run behind its own MAC, the abstraction was wrong and that is the finding.
It also unlocks line-rate receive (§5.4) and is the prerequisite for any
rate above 1 GbE.

*Gate: Phases 2 and 3 pass their own gates unchanged, with only the
adapter swapped.*

### Phase 5 — Reliable Connection, both directions (16–24 weeks)

ACK/NAK generation and consumption, PSN window tracking, the
retransmission buffer and its timers, duplicate detection, RNR-NAK, and
the receive-queue request handler. This is the third of the problem that
§3's file sizes measure. `rhdl-rule` carries the retry logic.

*Gate: `ib_write_bw` from `perftest` completes against the RHDL endpoint
with zero retransmission errors, and completes again with deliberately
injected loss.*

### Phase 6 — READ, SEND/RECV, completions (12–16 weeks)

An RDMA READ responder reads local memory, which Phase 3's `qp::mr`
already provides; what is new is the read-response generator and the
duplicate-read cache. Work-completion generation makes the endpoint
usable from ordinary verbs code.

### Phase 7 — Congestion control (8–12 weeks)

ECN marking, CNP generation and consumption, and the DCQCN rate-limiter
control loop. Only needed on a shared fabric; on a point-to-point link it
is dead logic. Sequence it when a real deployment needs it, and note that
the rate controller is a DSP control loop, which this repository is now
well equipped to build and test.

### Totals

| Target | Cumulative |
|---|---|
| Phase −1 — a measured packets-per-second number | one afternoon |
| Phase 0 — CRC-32, IPv4, UDP | 3–4 weeks |
| Phase 1 — verified parser/generator + link boundary | 7–10 weeks |
| **Phase 2 — UC WRITE send: samples in host GPU memory** | **13–18 weeks (3–4.5 months)** |
| **Phase 3 — UC WRITE receive** | **19–26 weeks (4.5–6 months)** |
| Phase 4 — own-MAC adapter, line-rate receive, >1 GbE possible | 24–33 weeks |
| Phase 5 — Reliable Connection | 40–57 weeks |
| Phase 6 — full verb set | 52–73 weeks |
| Phase 7 — congestion control | 60–85 weeks (14–20 months) |

Two things to note about these numbers versus the first draft. **Phase 2
got cheaper** — 13–18 weeks instead of 18–24 — because the hard GEM
removes `net::mac` from the critical path and drops the CRC-32 width
requirement from 512 bits to 8. **The full stack got slightly longer**,
because receive-direction WRITE and the own-MAC adapter are now explicit
phases rather than being folded into later ones. The full-stack figure
remains consistent with blue-rdma's 26 kLOC and with
`tier-c-flagship-cores.md`'s 18–24 months for comparable projects.

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

### Tier 8 — On hardware against a commodity NIC (Phase 2 exit)

A Red Pitaya on a direct link to a host with a ConnectX-class NIC, or any
NIC running `rxe`. Bandwidth and latency measured with `perftest`. **No
new board is required for this** — §4.3 — which means Tier 8 is reachable
in Phase 2 rather than being hardware-gated indefinitely.

The acceptance check has two steps, and the second is the actual goal:

1. The host application reads its own registered **host** buffer and
   finds the expected samples.
2. With `nvidia-peermem` loaded and the buffer from `cudaMalloc`, a CUDA
   kernel reads its own **GPU** memory and finds them. Host RAM is never
   touched.

### Tier 9 — Adapter equivalence (Phase 4 gate)

The test that §5.1 is real rather than aspirational: **Phases 2 and 3
pass their own gates with only `link::gem_ring` swapped for
`link::mac_phy`, and no change to any widget above the boundary.** If a
RoCE-layer widget needs editing to pass, the abstraction leaked and that
is the finding to report, not to paper over.

---

## 11 — Risks and open questions

1. **The width-generic datapath may not be expressible** in today's
   kernel subset (§8). Mitigated by starting byte-serial; the Phase 1
   write-up is the decision point, and the answer is an input to
   `kernel-language-extensions.md` either way.
2. **The PS descriptor rate is the one risk that can invalidate the
   architecture** (§4.3). Line rate on the GEM path needs ~113k
   packets/s retired by a 667 MHz Cortex-A9, and the PS GEM's 1522-byte
   frame ceiling caps PMTU at 1024 so the packet count cannot be reduced
   by using bigger frames. Mitigated by Phase −1, which measures it in
   an afternoon for free. Mitigated again by the real data rate, which
   after decimation is far below line rate.
3. **Receiving through the PS GEM puts the CPU in the data path**
   (§5.4). Unavoidable on that adapter: GEM DMAs into DDR and the PS
   must hand the buffer over. Fine for control and acknowledgements,
   wrong for high-rate receive. The mitigation is Phase 4's own-MAC
   adapter, and the reason §5.1's abstraction is a day-one requirement
   rather than a later generalisation.
4. **GPUDirect RDMA is a host procurement constraint, and consumer
   GeForce is not officially supported** (§4.5). It also needs
   ConnectX-3 VPI or newer, `nvidia-peermem` with kernel peer-memory
   support, and ideally NIC and GPU on one PCIe root complex. This is
   the most likely procurement surprise in the project and it should be
   verified with `perftest` on the intended host **before** Phase 2.
5. **UC is only safe on the topology we have.** If the instrument ever
   lands on a shared, congested fabric, the RC phase stops being
   optional and becomes a correctness requirement, not a performance
   one. Recorded here so it is not a surprise.
6. **The retransmission buffer is a bandwidth-delay product.** At 1 GbE
   it is block RAM; at 10G and above it is external memory and the
   project acquires a memory-controller dependency it does not have
   today.
7. **`rhdl-rule` has not been used at this scale.** Three pilot widgets
   is not a 26 kLOC core. Phase 2's send queue is the first real test,
   and discovering a rule-scheduler limitation there is a plausible
   outcome — which is a reason to do it, but the schedule should hold
   slack for it.
8. **Copyleft proximity — for the RoCE layer only.** Phase 0's
   reference, `verilog-ethernet`, is MIT and may be read freely. The
   RoCE-layer references are not: blue-rdma, RoCKS and Scapy are
   GPL-family. For those the discipline is read for architecture, use as
   oracles, write our own from the IBTA specification, and keep
   `roce-subset-spec.md` as the evidence of independent derivation.
9. **No reference implementation runs at 1 GbE** (§3). Every one runs
   at 10G or above. The byte-serial ICRC and the 8-bit segmentation path
   would be ours alone, with no prior art to diff against — which is why
   §10's Tier 6 and Tier 7 are not optional niceties but the only thing
   standing between us and a plausible-looking stack that no NIC accepts.

10. **An unoptimised 1 GbE datapath is 8 bits at 125 MHz.** That should
   close comfortably, but the ICRC and FCS paths are the long ones, and
   `auto-pipelining-plan.md` has not shipped. Expect hand-placed relay
   stations and note where they are.

---

## 12 — What is being asked for

This document commits nothing. The architecture question is now settled —
it runs on existing hardware (§4.3) — so what remains is scheduling and
two verifications.

**Do these two before any code:**

1. **Run Phase −1.** Blast pre-built 1086-byte frames from userspace on
   the Red Pitaya and count what the PS retires. One afternoon, no RHDL,
   and it is the only experiment that can invalidate the architecture.
2. **Verify the host side.** Confirm the intended PC's NIC and GPU
   support GPUDirect RDMA — ConnectX-3 VPI or newer, `nvidia-peermem`
   loadable, and a GPU on NVIDIA's support matrix (§4.5: consumer
   GeForce is not officially supported). `ib_write_bw` between two hosts,
   with `--use_cuda`, proves the receiving half independently of anything
   we build.

**Then three decisions:**

3. **Does this get scheduled**, given it is Tier-C-scale and competes
   with RV32I, the Alto and the Tier 0–4 widget library? Phase 0's
   widgets — CRC-32, IPv4, UDP — are on the roadmap on their own merits
   and are worth doing either way.
4. **Is the §9 staging agreed *as staging*?** CLAUDE.md's anti-sliver
   rule requires explicit sign-off on splitting work this size, and part
   of what is being agreed is that Phase 2 ships as "UC RDMA WRITE send"
   rather than as "RoCEv2."
5. **Is a `net` category in `rhdl-fpga` acceptable** (§6) for the
   IPv4/UDP widgets in Phase 0 and `net::mac` in Phase 4? This is an
   `architecture.md` question and is the one structural decision Phase 0
   cannot start without.

**And one comparison that should be made rather than assumed:**

6. **Is DOCA GPUNetIO rejected, and on what grounds?** (§4.6) It gets
   samples into GPU memory with *no* FPGA protocol work — the sensor
   sends plain UDP — at the cost of a host poll-mode driver on a
   dedicated core and owning the reassembly logic yourself. RoCEv2 wins
   on host CPU, on not writing reassembly, and on the §1 strategic
   grounds. If the goal is purely "samples in GPU memory, soon," that
   deserves a straight answer before committing 3–4.5 months.
