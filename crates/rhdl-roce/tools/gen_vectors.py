#!/usr/bin/env python3
"""Generate RoCEv2 RDMA WRITE reference vectors using Scapy as the oracle.

Scapy's `scapy.contrib.roce` is an independent implementation of the
RoCEv2 headers and of the Invariant CRC, used in practice to dissect
real RoCE traffic.  Using it as the oracle means agreement is evidence
about the protocol and not merely about our own reading of the spec.

Run:
    python3 -m venv venv && ./venv/bin/pip install scapy
    ./venv/bin/python tools/gen_vectors.py > tests/vectors/write_vectors.txt

The output is committed.  Tests only *check* it, so a full `cargo test`
leaves the tree clean (CLAUDE.md §8).  Regenerate deliberately.
"""
import struct
import sys

from scapy.contrib.roce import BTH
from scapy.all import Ether, IP, UDP, Raw, raw

SCAPY_VERSION_PIN = "2.8.0"

# --- the connection parameters, fixed across every vector -------------
SRC_MAC = "00:0a:35:01:02:03"      # a Xilinx OUI, i.e. the FPGA
DST_MAC = "ec:0d:9a:aa:bb:cc"      # a Mellanox OUI, i.e. the host NIC
SRC_IP = "192.168.1.50"
DST_IP = "192.168.1.10"
SPORT = 0x1234                      # RoCEv2 leaves the source port free
DPORT = 4791                        # IANA-assigned, fixed
TTL = 64
IP_ID = 0

# --- opcodes (transport base + operation), per IBTA ------------------
RC = 0x00
UC = 0x20
WRITE_FIRST, WRITE_MIDDLE, WRITE_LAST, WRITE_LAST_IMM = 0x06, 0x07, 0x08, 0x09
WRITE_ONLY, WRITE_ONLY_IMM = 0x0A, 0x0B


def reth(va, rkey, dma_len):
    """RDMA Extended Transport Header.  Not in scapy.contrib.roce, so it
    is built by hand: virtual address (64), R_Key (32), DMA length (32),
    all big-endian."""
    return struct.pack("!QII", va, rkey, dma_len)


def build(opcode, dqpn, psn, payload, *, va=None, rkey=None, dma_len=None,
          padcount=0, ackreq=0):
    body = b""
    if va is not None:
        body += reth(va, rkey, dma_len)
    body += payload
    pkt = (Ether(src=SRC_MAC, dst=DST_MAC)
           / IP(src=SRC_IP, dst=DST_IP, ttl=TTL, id=IP_ID)
           / UDP(sport=SPORT, dport=DPORT, chksum=0)
           / BTH(opcode=opcode, dqpn=dqpn, psn=psn, pkey=0xffff,
                 padcount=padcount, ackreq=ackreq)
           / Raw(body))
    return raw(pkt)


def icrc_input(frame, ip_off=14, udp_off=34, bth_off=42):
    """The exact byte sequence CRC-32 is computed over, per
    BTH.compute_icrc: 8 bytes of 0xFF, then the IP header with tos, ttl
    and checksum masked, the UDP header with its checksum masked, the
    BTH with its fecn/becn/resv6 byte masked, then everything up to but
    not including the 4-byte ICRC field."""
    ip_h = bytearray(frame[ip_off:ip_off + 20])
    ip_h[1] = 0xFF                    # tos / DSCP+ECN
    ip_h[8] = 0xFF                    # ttl
    ip_h[10:12] = b"\xff\xff"         # header checksum
    udp_h = bytearray(frame[udp_off:udp_off + 8])
    udp_h[6:8] = b"\xff\xff"          # udp checksum
    bth_h = bytearray(frame[bth_off:bth_off + 12])
    bth_h[4] = 0xFF                   # fecn | becn | resv6
    body = frame[bth_off + 12:-4]     # ext headers + payload, no ICRC
    return b"\xff" * 8 + bytes(ip_h) + bytes(udp_h) + bytes(bth_h) + body


VECTORS = []


def add(name, note, **kw):
    payload = kw.pop("payload")
    frame = build(payload=payload, **kw)
    VECTORS.append((name, note, kw, payload, frame))


VA = 0x0000_7F00_1234_0000
RKEY = 0x0000_A5A5

# A single-packet message: the common case for a small sensor burst.
add("rc_write_only_16", "RC RDMA_WRITE_ONLY, 16-byte payload",
    opcode=RC | WRITE_ONLY, dqpn=0x000111, psn=0x000001,
    va=VA, rkey=RKEY, dma_len=16, payload=bytes(range(16)))

# UC is the transport roce-v2-architecture.md §5.3 targets first.
add("uc_write_only_16", "UC RDMA_WRITE_ONLY -- the §5.3 target transport",
    opcode=UC | WRITE_ONLY, dqpn=0x000111, psn=0x000001,
    va=VA, rkey=RKEY, dma_len=16, payload=bytes(range(16)))

# Zero-length write: legal, and the degenerate case for the segmenter.
add("uc_write_only_0", "UC RDMA_WRITE_ONLY, zero-length payload",
    opcode=UC | WRITE_ONLY, dqpn=0x000111, psn=0x000002,
    va=VA, rkey=RKEY, dma_len=0, payload=b"")

# Not a multiple of four, so padcount is non-zero.  This is the ragged
# tail that §8 calls the real language risk.
add("uc_write_only_13_pad3", "UC RDMA_WRITE_ONLY, 13 bytes -> padcount 3",
    opcode=UC | WRITE_ONLY, dqpn=0x000111, psn=0x000003, padcount=3,
    va=VA, rkey=RKEY, dma_len=13, payload=bytes(range(13)) + b"\x00\x00\x00")

# A three-packet message at PMTU 256: FIRST carries the RETH, MIDDLE and
# LAST do not, and the PSN advances by one each time.
add("uc_write_first_256", "UC RDMA_WRITE_FIRST of a 600-byte message, PMTU 256",
    opcode=UC | WRITE_FIRST, dqpn=0x000111, psn=0x000010,
    va=VA, rkey=RKEY, dma_len=600,
    payload=bytes((i * 7 + 1) & 0xFF for i in range(256)))
add("uc_write_middle_256", "UC RDMA_WRITE_MIDDLE -- no RETH",
    opcode=UC | WRITE_MIDDLE, dqpn=0x000111, psn=0x000011,
    payload=bytes((i * 7 + 1) & 0xFF for i in range(256, 512)))
add("uc_write_last_88", "UC RDMA_WRITE_LAST, ragged final packet -- no RETH",
    opcode=UC | WRITE_LAST, dqpn=0x000111, psn=0x000012,
    payload=bytes((i * 7 + 1) & 0xFF for i in range(512, 600)))

# A different QP / PSN / rkey, so nothing can accidentally hard-code them.
add("rc_write_only_alt_ids", "RC RDMA_WRITE_ONLY with unrelated QPN/PSN/rkey/VA",
    opcode=RC | WRITE_ONLY, dqpn=0xABCDEF, psn=0xFFFFFE, ackreq=1,
    va=0xDEAD_BEEF_0000_1000, rkey=0x1234_5678, dma_len=4,
    payload=b"\xde\xad\xbe\xef")


# ---------------------------------------------------------------------
# Differential fuzz corpus
# ---------------------------------------------------------------------
#
# The eight curated vectors above pin the shapes that matter. They do
# not explore the input space, and that gap is not theoretical: a
# mutation deleting the IPv4 checksum's second carry fold survived every
# one of them, because none of the eight drives the accumulator into the
# window where the first fold carries.
#
# This generates a large corpus of randomised frames from a fixed seed
# and lets Scapy -- an independent implementation -- say what each one's
# bytes should be. The corpus is committed, so the Rust side is
# deterministic and needs no Python at test time.
#
# Records are compact on purpose: the payload is *derived* from a seed
# by a formula both sides compute, so a 4 KB payload costs 8 characters
# in the file rather than 8192.

FUZZ_SEED = 0x5EED_1234
FUZZ_COUNT = 2000


class Lcg:
    """A tiny deterministic generator, so the corpus is reproducible
    without depending on Python's `random` implementation details."""

    def __init__(self, seed):
        self.s = seed & 0xFFFFFFFF

    def next(self):
        # Numerical Recipes LCG constants.
        self.s = (self.s * 1664525 + 1013904223) & 0xFFFFFFFF
        return self.s

    def below(self, n):
        return self.next() % n

    def bits(self, n):
        """n random bits.  The obvious loop is easy to get wrong -- an
        earlier version returned 16 bits for n=48, which showed up as
        suspiciously small MAC addresses in the corpus."""
        v = 0
        got = 0
        while got < n:
            v = (v << 32) | self.next()
            got += 32
        return v & ((1 << n) - 1)


def payload_byte(seed, i):
    """The formula both the generator and the Rust test use, so the
    payload need not be stored."""
    return (seed + i * 31 + (i >> 3) * 7) & 0xFF


def derive_payload(seed, data_len, pad):
    return bytes(payload_byte(seed, i) for i in range(data_len)) + b"\x00" * pad


FUZZ_OPS = [
    ("First", WRITE_FIRST), ("Middle", WRITE_MIDDLE), ("Last", WRITE_LAST),
    ("LastWithImmediate", WRITE_LAST_IMM), ("Only", WRITE_ONLY),
    ("OnlyWithImmediate", WRITE_ONLY_IMM),
]


def gen_fuzz(out):
    rng = Lcg(FUZZ_SEED)
    out.write("# --- differential fuzz corpus ---\n")
    out.write(f"# {FUZZ_COUNT} randomised frames, LCG seed {FUZZ_SEED:#010x}.\n")
    out.write("# `payload` is NOT stored: byte i is\n")
    out.write("#   (payload_seed + i*31 + (i>>3)*7) & 0xFF\n")
    out.write("# for i < data_len, then `pad` zero bytes.  Only the header\n")
    out.write("# block and the ICRC are recorded, which is what the frame\n")
    out.write("# generator and the ICRC engine actually produce.\n")
    out.write("#\n")
    out.write("# One case per line, space separated, in this column order:\n")
    out.write("#   name op opcode src_mac dst_mac src_ip dst_ip sport\n")
    out.write("#   ip_id ttl dqpn psn ackreq has_reth va rkey dma_len\n")
    out.write("#   data_len pad payload_seed header_hex icrc_hex\n")
    out.write(f"corpus_size = {FUZZ_COUNT}\n\n")

    for n in range(FUZZ_COUNT):
        op_name, op = FUZZ_OPS[rng.below(len(FUZZ_OPS))]
        transport = UC if rng.below(2) else RC
        has_reth = op in (WRITE_FIRST, WRITE_ONLY, WRITE_ONLY_IMM)
        # FIRST and MIDDLE always carry a whole MTU, so never pad.
        if op in (WRITE_FIRST, WRITE_MIDDLE):
            data_len = 4 * rng.below(257)
        else:
            data_len = rng.below(1025)
        pad = (4 - data_len % 4) % 4
        payload_seed = rng.below(256)
        payload = derive_payload(payload_seed, data_len, pad)

        # Uniform random never reaches the IPv4 checksum's second carry
        # fold: `acc` has to land in one of about twenty values out of a
        # ~450,000 range, so 2000 uniform cases hit it zero times --
        # measured, not assumed. A fuzzer that does not know where the
        # cliffs are will not find them, so every tenth case is *solved*
        # into the window by choosing `ip_id` to suit.
        force_carry = (n % 10) == 7

        src_mac = rng.bits(48)
        dst_mac = rng.bits(48)
        src_ip = rng.bits(32)
        dst_ip = rng.bits(32)
        sport = rng.bits(16)
        ip_id = rng.bits(16)
        ttl = rng.below(256)
        dqpn = rng.bits(24)
        psn = rng.bits(24)
        ackreq = rng.below(2)
        va = rng.bits(64)
        rkey = rng.bits(32)
        dma_len = rng.bits(32) if has_reth else 0

        def mac(v):
            b = v.to_bytes(6, "big")
            return ":".join(f"{x:02x}" for x in b)

        def ip(v):
            return ".".join(str(x) for x in v.to_bytes(4, "big"))

        if force_carry:
            # total_length is 20 IP + 8 UDP + 12 BTH + 4 ICRC + RETH? + padded
            totlen = 44 + (16 if has_reth else 0) + data_len + pad
            fixed = (0x4500 + totlen + ((ttl << 8) | 17)
                     + (src_ip >> 16) + (src_ip & 0xFFFF)
                     + (dst_ip >> 16) + (dst_ip & 0xFFFF))
            # fold1 carries out of bit 15 only for these accumulator values
            for k in range(1, 7):
                for target in range((k + 1) * 0x10000 - k, (k + 1) * 0x10000):
                    cand = target - fixed
                    if 0 <= cand <= 0xFFFF:
                        ip_id = cand
                        break
                else:
                    continue
                break

        body = (struct.pack("!QII", va, rkey, dma_len) if has_reth else b"") + payload
        pkt = (Ether(src=mac(src_mac), dst=mac(dst_mac))
               / IP(src=ip(src_ip), dst=ip(dst_ip), ttl=ttl, id=ip_id)
               / UDP(sport=sport, dport=DPORT, chksum=0)
               / BTH(opcode=transport | op, dqpn=dqpn, psn=psn, pkey=0xffff,
                     padcount=pad, ackreq=ackreq)
               / Raw(body))
        frame = raw(pkt)
        hdr_len = 54 + (16 if has_reth else 0)

        # One positional line per case; the column order is documented
        # in the corpus header. Key=value for 22 fields x 2000 cases
        # doubled the file size for no added clarity in a generated file.
        out.write(
            " ".join(
                [
                    f"fuzz{n:05d}",
                    op_name,
                    f"{transport | op:#04x}",
                    f"{src_mac:#014x}",
                    f"{dst_mac:#014x}",
                    f"{src_ip:#010x}",
                    f"{dst_ip:#010x}",
                    f"{sport:#06x}",
                    f"{ip_id:#06x}",
                    str(ttl),
                    f"{dqpn:#08x}",
                    f"{psn:#08x}",
                    str(ackreq),
                    str(1 if has_reth else 0),
                    f"{va:#018x}",
                    f"{rkey:#010x}",
                    str(dma_len),
                    str(data_len),
                    str(pad),
                    str(payload_seed),
                    frame[:hdr_len].hex(),
                    frame[-4:].hex(),
                ]
            )
            + "\n"
        )


def main():
    out = sys.stdout
    out.write("# RoCEv2 RDMA WRITE reference vectors.\n")
    out.write("# GENERATED by tools/gen_vectors.py -- do not edit by hand.\n")
    out.write(f"# Oracle: scapy {SCAPY_VERSION_PIN} (scapy.contrib.roce).\n")
    out.write("#\n")
    out.write("# Records are separated by blank lines; keys are `name = value`.\n")
    out.write("# `frame` is the complete Ethernet frame without the FCS (the\n")
    out.write("# MAC appends that).  `icrc_input` is the exact byte sequence\n")
    out.write("# CRC-32 is computed over, so a mismatch in `icrc` can be\n")
    out.write("# localised to either the masking or the CRC itself.\n")
    out.write("#\n")
    out.write(f"# src_mac = {SRC_MAC}\n# dst_mac = {DST_MAC}\n")
    out.write(f"# src_ip = {SRC_IP}\n# dst_ip = {DST_IP}\n")
    out.write(f"# sport = {SPORT:#06x}\n# dport = {DPORT}\n")
    out.write(f"# ttl = {TTL}\n# ip_id = {IP_ID}\n# pkey = 0xffff\n\n")
    for name, note, kw, payload, frame in VECTORS:
        ic_in = icrc_input(frame)
        out.write(f"name = {name}\n")
        out.write(f"note = {note}\n")
        # The connection parameters, repeated per record so a test does
        # not have to parse the header comments or hard-code them.
        out.write(f"src_mac = 0x{SRC_MAC.replace(':', '')}\n")
        out.write(f"dst_mac = 0x{DST_MAC.replace(':', '')}\n")
        out.write("src_ip = 0x%02x%02x%02x%02x\n" % tuple(int(x) for x in SRC_IP.split(".")))
        out.write("dst_ip = 0x%02x%02x%02x%02x\n" % tuple(int(x) for x in DST_IP.split(".")))
        out.write(f"sport = {SPORT:#06x}\n")
        out.write(f"ip_id = {IP_ID:#06x}\n")
        out.write(f"ttl = {TTL}\n")
        out.write(f"opcode = {kw['opcode']:#04x}\n")
        out.write(f"dqpn = {kw['dqpn']:#08x}\n")
        out.write(f"psn = {kw['psn']:#08x}\n")
        out.write(f"padcount = {kw.get('padcount', 0)}\n")
        out.write(f"ackreq = {kw.get('ackreq', 0)}\n")
        has_reth = kw.get("va") is not None
        out.write(f"has_reth = {1 if has_reth else 0}\n")
        if has_reth:
            out.write(f"va = {kw['va']:#018x}\n")
            out.write(f"rkey = {kw['rkey']:#010x}\n")
            out.write(f"dma_len = {kw['dma_len']}\n")
        out.write(f"payload_len = {len(payload)}\n")
        out.write(f"payload = {payload.hex()}\n")
        out.write(f"icrc_input = {ic_in.hex()}\n")
        out.write(f"icrc = {frame[-4:].hex()}\n")
        out.write(f"frame = {frame.hex()}\n")
        out.write("\n")
    gen_fuzz(out)


if __name__ == "__main__":
    main()
