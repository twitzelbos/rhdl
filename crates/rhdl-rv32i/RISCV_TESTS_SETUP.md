# Setting up the upstream `riscv-tests` corpus

The `tests/upstream_riscv_tests.rs` file runs the RISC-V Foundation's
official `rv32ui-p-*` test suite — the canonical conformance corpus
maintained at <https://github.com/riscv-software-src/riscv-tests>.

These tests aren't part of the repo (each ELF is ~10 KB and would
balloon the git history); instead the harness expects pre-built ELFs
at a well-known path.  This document walks through building them.

## What you need

- A RISC-V cross compiler (the `riscv64-elf-gcc` toolchain works
  for both rv32 and rv64 targets).
- `git`, `make`, a working build environment.

## Install the toolchain

| Platform | Command |
|----------|---------|
| macOS    | `brew install riscv64-elf-gcc` |
| Ubuntu / Debian | `sudo apt install gcc-riscv64-unknown-elf` (or build from <https://github.com/riscv-collab/riscv-gnu-toolchain>) |
| Fedora / RHEL   | `sudo dnf install riscv64-elf-gcc` |
| Arch Linux      | `paru -S riscv64-elf-gcc-bin` (AUR) |

After install, `riscv64-elf-gcc --version` should report something
like `riscv64-elf-gcc (GCC) 16.x` or newer.

The harness uses the `riscv64-elf-` prefix; if your toolchain uses
a different prefix (e.g. `riscv64-unknown-elf-`), pass it via the
`RISCV_PREFIX` make variable below.

## Build the rv32ui-p-* suite

```sh
# 1. Clone the upstream tests repo (with submodules — the test
#    environment is a separate submodule).
mkdir -p /tmp/riscv-tests-build
cd /tmp/riscv-tests-build
git clone --depth 1 --recurse-submodules https://github.com/riscv-software-src/riscv-tests.git
cd riscv-tests/isa

# 2. Build only the -p (physical/bare-metal) variants.  The -v
#    (virtual-memory) variants need a libc and won't work with a
#    minimal toolchain; we only need -p anyway.
#
#    Build a few first to verify the toolchain works:
make XLEN=32 RISCV_PREFIX=riscv64-elf- rv32ui-p-add rv32ui-p-addi

# 3. Build the full set (the rv32ui Makefrag lists 42 tests):
make XLEN=32 RISCV_PREFIX=riscv64-elf- $(echo rv32ui-p-{simple,add,addi,and,andi,auipc,beq,bge,bgeu,blt,bltu,bne,fence_i,jal,jalr,lb,lbu,ld_st,lh,lhu,lui,lw,ma_data,or,ori,sb,sh,sll,slli,slt,slti,sltiu,sltu,sra,srai,srl,srli,st_ld,sub,sw,xor,xori})
```

After the build, `/tmp/riscv-tests-build/riscv-tests/isa/`
should contain 42 ELF files matching `rv32ui-p-*` (each ~10 KB).
The harness checks for `rv32ui-p-add` as a canary.

## Running the tests

```sh
# RECOMMENDED on machines with limited RAM (each test runs the
# simulator in its own thread; default parallelism can stress
# memory):
cargo test -p rhdl-rv32i --test upstream_riscv_tests -- --test-threads=2

# Default parallelism (one thread per CPU):
cargo test -p rhdl-rv32i --test upstream_riscv_tests
```

If the ELFs aren't found at the expected path, **all** tests in
this file skip with a clear message pointing at this doc.
The rest of the `rhdl-rv32i` test suite continues to run normally.

## Why this is worth setting up

These tests are the canonical RISC-V conformance corpus.  Each
test (e.g. `rv32ui-p-add`) typically contains 30+ sub-test
sequences (`test_2`, `test_3`, ..., `test_N`) probing corner
cases of one specific instruction:

- Operand-ordering (rs1 vs rs2 swapped — caught one ALU bug we'd
  missed in PR #28's hand-translated suite).
- Immediate sign-extension at the boundary (LUI with imm
  `0x80000`, ADDI with imm `-2048`).
- Register-aliasing edge cases (RAW/WAR with rs1/rs2/rd all the
  same register).
- Data-hazard patterns specifically chosen to stress forwarding /
  load-use stall paths.

These are the bugs we'd never think to test for ourselves.  The
official suite has had 10+ years of community refinement;
disagreement is much more likely to indicate a real bug than a
test-suite issue.

## What this corpus catches that our prior validation didn't

Initial run (before the simulator's sub-word memory fix that
landed in this PR) failed 9/42 tests — **all** of them in the
sub-word load/store family (LB/LBU/LH/LHU/SB/SH plus the
load/store-interaction tests).  Our prior tests only used LW/SW
(word-aligned), so this entire class of bug had been invisible.
The sim fix (proper byte-addressed read-modify-write for sub-word
ops) brought us to 40/42.

The remaining 2 (`rv32ui-p-ld_st`, `rv32ui-p-ma_data`) exercise
edge cases involving combined misaligned + sub-word access that
need additional simulator work.  Documented as follow-ups; current
40/42 = 95.2% pass rate is far stronger than what we shipped before.

## Troubleshooting

### `riscv64-elf-gcc: command not found`

Install the toolchain per the table above.

### Build fails on `rv32ui-v-*` targets

The -v (virtual memory) variants need a C library (`string.h`,
`stdint.h`).  Skip them; we only use -p.  If `make rv32ui` (the
group target) fails on a -v variant, build the -p variants
explicitly as shown in step 3.

### `RISCV_PREFIX` mismatch

Some toolchains use `riscv64-unknown-elf-` instead of
`riscv64-elf-`.  Pass `RISCV_PREFIX=riscv64-unknown-elf-` to the
make command in step 2/3.
