# ShellSC

![Rust](https://img.shields.io/badge/Rust-1.70%2B-orange)
![License](https://img.shields.io/badge/License-BSD%203--Clause-blue)
![Platform](https://img.shields.io/badge/Platform-ARM%20%7C%20x86-green)

Shell script compiler for Android and Linux. Compiles `.sh` scripts into standalone ELF executables.

## Requirements

- **Rust** (1.70+)
- On Termux: `pkg install rust`

## Supported Platforms

- **ARMv7** (32-bit ARM — Android phones, Termux)
- **AArch64** (64-bit ARM — Android, Raspberry Pi, ARM servers)
- **x86_64** (PC Linux)
- **i686** (32-bit PC Linux)

Build and run on the same architecture: the stub is compiled for your host arch during `cargo build`, and packed binaries run on that arch.

## Install

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

- XOR encryption of bytecode with random per-build key
- Per-build opcode shuffling (Fisher-Yates)
- Control-flow flattening + opaque predicates
- Virtualized dispatch obfuscation
- Self-modifying bytecode (`--smc`): decrypt one instruction at a time, with three anti-decode layers:
  - **Chained keystream** — each instruction's keystream derives from the decoded bytes of the previous one, so decryption must replay from instruction 0 in order (no parallel/sliced unpacking)
  - **Runtime salt** — const-pool strings, heredoc bodies and function names are masked with a per-process entropy value that exists only in the live process, never in the `.sc` file; static decryption with the correct key still yields garbage
  - **Polymorphic mixers** — the keystream generator itself is picked per build from 4 structurally different algorithm shapes, so two builds of the same script run different decode algorithms
- Anti-debug (ptrace check), anti-hook (LD_PRELOAD/Frida detection)
- Self-debug (`--self-debug`): fork a child that ptrace-attaches the parent, occupying the single tracer slot so no external debugger can attach
- CRC32 integrity verification
- **Key derivation from stub `.text`** — the XOR key is masked with the CRC32 of the stub's own `.text` section and re-derived at runtime from the mapped ELF; patching a single byte inside `.text` (e.g. NOP-ing an anti-debug check) invalidates the key and the binary refuses to run

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

---

[#ShellSC](https://github.com/search?q=ShellSC) · [#RustLang](https://github.com/search?q=RustLang) · [#ShellScripting](https://github.com/search?q=ShellScripting) · [#Obfuscation](https://github.com/search?q=Obfuscation) · [#AntiDebugging](https://github.com/search?q=AntiDebugging) · [#ReverseEngineering](https://github.com/search?q=ReverseEngineering) · [#CyberSecurity](https://github.com/search?q=CyberSecurity) · [#LinuxDev](https://github.com/search?q=LinuxDev) · [#AndroidSecurity](https://github.com/search?q=AndroidSecurity) · [#Compiler](https://github.com/search?q=Compiler) · [#OpenSource](https://github.com/search?q=OpenSource) · [#GitHub](https://github.com/search?q=GitHub)

