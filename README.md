# ShellSC

Shell script compiler for Android and Linux. Compiles `.sh` scripts into standalone ELF executables.

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

</content>