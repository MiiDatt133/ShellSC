# ShellSC

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

Pipelines, subshells, brace groups, `if/elif/else`, `while/until/for/case`, functions, arrays (`arr=(a b)`, `${arr[@]}`, `${#arr[@]}`), command substitution `$(cmd)`, arithmetic `$((expr))`, heredocs, here-strings, glob/brace expansion, parameter expansion (`:-`, `:=`, `:+`, `:?`, `#pat`, `%pat`, `##pat`, `%%pat`, substring), `trap`, `eval`, `exec`, `local`, redirects (`>`, `>>`, `<`, `2>&1`, etc.), background `&`, boolean `&&`/`||`.

## Protection (`--protect`)

- XOR encryption of bytecode with random per-build key
- Per-build opcode shuffling (Fisher-Yates)
- Control-flow flattening + opaque predicates
- Virtualized dispatch obfuscation
- Self-modifying bytecode (`--smc`): decrypt one instruction at a time
- Anti-debug (ptrace check), anti-hook (LD_PRELOAD/Frida detection)
- CRC32 integrity verification

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

#ShellSC #RustLang #ShellScripting #Obfuscation #AntiDebugging #ReverseEngineering #CyberSecurity #LinuxDev #AndroidSecurity #Compiler #OpenSource #GitHub

