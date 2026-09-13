# ShellSC

![Rust](https://img.shields.io/badge/Rust-1.70%2B-orange)
![License](https://img.shields.io/badge/License-BSD%203--Clause-blue)
![Platform](https://img.shields.io/badge/Platform-ARM%20%7C%20x86-green)

Shell script compiler for Android and Linux. Compiles `.sh` scripts into standalone ELF executables.

## Requirements

- **Rust 1.70+** to build from source
- On Termux: `pkg install rust`

## Supported Platforms

- **ARMv7** (32-bit ARM — Android phones, Termux)
- **AArch64** (64-bit ARM — Android, Raspberry Pi, ARM servers)
- **x86_64** (PC Linux)
- **i686** (32-bit PC Linux)

Build and run on the same architecture: the stub is compiled for your host arch during `cargo build`, and packed binaries run on that arch.

## Install

Download the tarball for your CPU from
[Releases](https://github.com/MiiDatt133/ShellSC/releases) — this line
picks the right one automatically:

```sh
curl -LO https://github.com/MiiDatt133/ShellSC/releases/latest/download/shellsc-$( \
  case "$(uname -m)" in \
    armv7l|armv6l) echo armv7a ;; \
    aarch64|arm64)  echo aarch64 ;; \
    x86_64)         echo x86_64 ;; \
    i686|i386)      echo i686 ;; \
  esac).tar.gz
tar -xzf shellsc-*.tar.gz
cp shellsc-*/shellsc $PREFIX/bin/
cp -r shellsc-*/stubs $PREFIX/bin/
shellsc build script.sh
```

On PC Linux use `~/.local/bin` instead of `$PREFIX/bin` (make sure it is in `PATH`).

Or build from source:

```sh
git clone https://github.com/MiiDatt133/ShellSC.git
cd ShellSC
cargo build --release
```

Binary at `target/release/shellsc`.

## Usage

```sh
# Compile script → .sc binary
./target/release/shellsc build script.sh

# Run
chmod +x script.sc && ./script.sc

# With protection (encrypted bytecode, anti-debug, SMC)
./target/release/shellsc build --protect --smc script.sh

# With all protections at once (protect + smc + self-debug)
./target/release/shellsc build --all script.sh
```

## Commands

| Command | Description |
|---------|-------------|
| `build <file.sh>` | Compile to `.sc` executable |
| `gen <file.sh>` | Emit `.sbc` bytecode assembly |
| `pack <file.sbc>` | Pack `.sbc` into `.sc` ELF |
| `dump-ast <file.sh>` | Print parsed AST |
| `dump-ir <file.sh>` | Print IR ops |
| `disasm <file.sbc>` | Disassemble bytecode |

## Supported Syntax

Pipelines, subshells, brace groups, `if/elif/else`, `while/until/for/case`, functions, arrays (`arr=(a b)`, `${arr[@]}`, `${#arr[@]}`, `arr[i]=v`, `arr+=(...)`, `unset arr[i]`, `read -a arr`), command substitution `$(cmd)`, process substitution `<(cmd)` (read mode, e.g. `diff <(a) <(b)`, `read x < <(cmd)`, `p=<(cmd)`), arithmetic `$((expr))`, heredocs, here-strings, glob/brace expansion, ANSI-C quoting `$'\t'` / `$'\x41'`, parameter expansion (`:-`, `:=`, `:+`, `:?`, `#pat`, `%pat`, `##pat`, `%%pat`, substring), tilde expansion (`~`, `~/path`), `trap` (string or function name, incl. `trap myfn EXIT`), `eval`, `exec` (incl. fd>2 persist `exec 3>&1`, `exec 3>file`), `local`, `shift`, redirects (`>`, `>>`, `<`, `2>&1`, `&>`/`&>>` both-streams, `>&N` fd>2 dup, etc.), background `&` (external commands and shell functions), boolean `&&`/`||`.

Not supported: write-mode process substitution `>(cmd)`, associative arrays (`declare -A`), coprocesses.

## Protection (`--protect`)

Available on `build` and `pack`. Use `--all` to enable every protection at once (`--protect --smc --self-debug`). A protected `.sc` prints `Protected by ShellSC` as its first line when run.

- XOR encryption of bytecode with per-build random key
- Per-build opcode shuffling (Fisher-Yates)
- Per-build stub variants — every protected build rebuilds the stub with fresh guard constants
- Bytecode obfuscation passes, applied before sealing:
  - Bogus control flow — a subset of jumps routes through appended decoy junk blocks
  - Instruction substitution — long pool strings split into `PushConst(a) PushConst(b) ConcatN(2)`
- Control-flow flattening + opaque predicates
- Virtualized dispatch obfuscation
- Seeded-header masking (v4) — seeds stored XOR-masked with the stub `.text` CRC
- Self-modifying bytecode (`--smc`) — decrypt one instruction at a time; chained keystream, per-process runtime salt, keystream generator picked per build from 4 shapes
- Anti-debug (TracerPid check) and anti-hook (LD_PRELOAD, hook-framework markers in `/proc/self/maps`), read via raw syscalls — a preloaded library cannot interpose them; LD_PRELOAD is read from the kernel's `/proc/self/environ` snapshot, which `unsetenv()` cannot clean
- Self-debug (`--self-debug`) — child process ptrace-attaches the parent, taking the tracer slot
- CRC32 integrity verification
- Key derivation from stub `.text` — patching one byte of `.text` invalidates the key

Note: a protected build compiles a fresh stub variant when a source workspace is present (build from source). With a prebuilt shellsc, `--protect` uses the bundled stub directly: same encryption and checks, without the per-build stub code variation.

## Structure

```
crates/
  shell_lex/    — lexer
  shell_parse/  — parser (AST)
  shell_ir/     — lowering (IR + builtins)
  shell_bc/     — bytecode assembly
  shell_vm/     — runtime VM
  shell_pack/   — ELF packing
  shell_stub/   — stub loader
  shellsc/      — CLI
```

