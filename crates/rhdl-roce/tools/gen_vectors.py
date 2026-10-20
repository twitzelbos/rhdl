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


if __name__ == "__main__":
    main()
