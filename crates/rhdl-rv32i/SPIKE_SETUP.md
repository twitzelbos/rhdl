# Setting up Spike (riscv-isa-sim) for `rhdl-rv32i` lockstep tests

The Spike-lockstep test file (`tests/spike_lockstep.rs`) validates
both of our hardware cores against the **official RISC-V ISA
reference simulator**, [`riscv-isa-sim`][upstream] (a.k.a. Spike),
maintained by the RISC-V Software organization at
<https://github.com/riscv-software-src/riscv-isa-sim>.

[upstream]: https://github.com/riscv-software-src/riscv-isa-sim

Spike is **not on Homebrew** as of this writing (the `spike` formula
in Homebrew is the LEGO product, not the RISC-V simulator), so it
must be built from source.  This document walks through the steps.

## What you need

- A C++17-capable compiler (tested with Apple Clang on macOS 14+,
  GCC 11+ on Linux).
- `git` for cloning the repository.
- `make` and a working build environment (Xcode CLT on macOS,
  build-essentials on Debian/Ubuntu).
- The `dtc` (device-tree compiler) package — Spike's configure
  script requires it as a hard dependency.

## Install device-tree-compiler

| Platform | Command |
|----------|---------|
| macOS    | `brew install dtc` |
| Ubuntu / Debian | `sudo apt install device-tree-compiler` |
| Fedora / RHEL   | `sudo dnf install dtc` |
| Arch Linux      | `sudo pacman -S dtc` |

## Build Spike from source

```sh
# 1. Clone (depth 1 is enough; we don't need history).
git clone --depth 1 https://github.com/riscv-software-src/riscv-isa-sim.git
cd riscv-isa-sim

# 2. Configure with an install prefix.  We use /tmp/spike-install
#    so it doesn't pollute system directories; you can use
#    /usr/local or anywhere else writable.
mkdir -p build && cd build
../configure --prefix=/tmp/spike-install

# 3. Build (≈5 min on an M-series Mac with -j8; longer elsewhere).
make -j$(nproc 2>/dev/null || sysctl -n hw.logicalcpu)

# 4. Install (creates /tmp/spike-install/bin/spike).
make install
```

After this, `spike --help` should produce something like:

```
Spike RISC-V ISA Simulator 1.1.1-dev
usage: spike [host options] <target program> [target options]
...
```

## Putting Spike on PATH (optional)

By default, the test harness checks two locations:

1. `which spike` (i.e., your `$PATH`)
2. `/tmp/spike-install/bin/spike` (our default install location)

If you installed Spike somewhere else, either:

- Add it to your `$PATH`:

  ```sh
  export PATH=/path/to/spike-install/bin:$PATH
  ```

- Or symlink it to one of the discovered locations:

  ```sh
  ln -s /your/spike/path /tmp/spike-install/bin/spike
  ```

## Running the tests

```sh
# RECOMMENDED on machines with <32 GB of RAM:
cargo test -p rhdl-rv32i --test spike_lockstep -- --test-threads=2

# Default parallelism (one thread per CPU; OK if you have lots of RAM):
cargo test -p rhdl-rv32i --test spike_lockstep
```

The first test (`spike_is_available`) prints the resolved Spike
path.  All other tests in this file run programs through Spike +
both hardware cores and assert the resulting memory state matches.

If Spike is not found, **all** Spike-lockstep tests skip with a
clear message.  The rest of the `rhdl-rv32i` test suite continues
to run normally.

### ⚠️ OOM warning

The Spike test suite has 300+ tests.  Each test spawns a Spike
subprocess, an ELF builder, and runs both hardware cores.  Default
`cargo test` parallelism is one thread per CPU (8-12 on modern
Macs), so up to ~12 of these run concurrently — pushing memory
pressure into the swap region on machines with ≤16 GB of RAM.

We hit a **kernel watchdog timeout** (system-wide hang requiring
a hard reboot) on a 16 GB M-series Mac during one development
run.  The crash logs implicate `kernel_task` running out of free
pages (`Compressor Info: 100% of segments limit (BAD)`).

**On any machine with ≤32 GB of RAM, run with `--test-threads=2`
(or `=1` to be safe).**  The full Spike suite still finishes in
under a minute single-threaded.

## Why we ship our own minimal ELF builder

Spike requires its input as an ELF file (no raw-binary loader).
Our test harness rolls its own minimal RV32 ELF builder
(`build_elf` in `tests/spike_lockstep.rs`) to avoid pulling in a
heavy ELF crate as a dev-dependency.  ~150 lines of plain Rust;
see the file for details.

## Troubleshooting

### `configure: error: device-tree-compiler not found`

Install `dtc` per the table above.

### Tests print `spike not found ... skipping`

Spike isn't on PATH and isn't at `/tmp/spike-install/bin/spike`.
Either add it to PATH or symlink it to that location.

### Tests print `spike output parse failed`

Spike was found, but it failed to load or run the test ELF.  This
usually means:

- A Spike version mismatch (check `spike --help` for version; we
  tested with 1.1.1-dev).  If you have a much older Spike, the
  `untiln pc 0` debug command may not exist; build a newer one.
- A test wrote to memory outside the mapped region.  Spike's
  default `-m1` (1 MiB at `0x80000000`) is matched by our ELF's
  `p_memsz = 0x10_0000`.  If you're adding a test that writes to
  addresses above `0x80100000`, increase `memsz` in `build_elf`.

### The Spike build fails on Apple Silicon

Make sure you have the Xcode Command Line Tools installed
(`xcode-select --install`).  As of this writing, `spike` builds
cleanly on macOS 14+ with Apple Clang 15.  Linux x86_64 builds
work with GCC 11+.

## Why this is worth setting up

Our other tests (compliance suite, fuzz, lockstep against our own
Rust simulator) cover a lot, but they all share our decoder.  If
the decoder has a bug, every test that uses our hardware will
inherit it.  Spike has its own decoder, its own execution engine,
and is the official RISC-V Foundation reference for compliance.

Disagreement between Spike and our hardware is a real bug — the
kind of bug we'd otherwise ship.  The Spike-lockstep tests in this
file have caught **(none yet, knock on wood)** during development,
which we read as encouraging given the breadth of `rhdl-rv32i`'s
existing self-validation.  But the value is exactly proportional
to the breadth of the Spike test corpus, which is why we keep
adding more (see `tests/spike_lockstep.rs` for the current set).
