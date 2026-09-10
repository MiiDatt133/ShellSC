# Divergences — difftest round "d" (arrays focus)

Date: 2026-09-07. Compiler: `./target/release/shellsc build difftest/dN.sh -o difftest/dN.sc`.
Run: `bash difftest/dN.sh` vs `env -u LD_PRELOAD ./difftest/dN.sc`.
Output pairs kept in `difftest/div-d/dN.{bash,sc}.out`.
All runtime divergences below have exit codes bash=0 / sc=0 (or as noted).

## Runtime divergences

| # | Script | Summary | Exit (bash/sc) | Diagnosis |
|---|--------|---------|-----------------|-----------|
| D-01 | d1.sh | `echo "$arr"` (array in plain scalar expansion, quoted) → bash prints `a` (element 0), sc prints empty | 0/0 | VM/IR: `$arr` lookup doesn't fall back to element 0 of array when name is an array |
| D-02 | d2.sh | `for x in "${files[*]}"` → bash one iteration `star:[one two]` (IFS-joined), sc two iterations (`star:[one]`,`star:[two]`). Also `for x in $*` after `set --`: sc doesn't split on IFS | 0/0 | Lowering/VM: `[*]` form treated same as `[@]` — no IFS-join for `for`-list; unquoted `$*` not word-split |
| D-03 | d4.sh | `local larr=(l1 l2)` inside function → sc executes `l1` as command ("l1: command not found"); local array assignment broken; `echo "in:${larr[@]}"` empty | 0/0 | Parser/lowering: `local arr=(...)` array-literal not supported for local; treated as command words |
| D-04 | d8.sh | Array expansions inside heredoc body all empty (`1:` instead of `1:alpha`, `count:` instead of `2`); scalar vars in heredoc work (`mixed:world/`) | 0/0 | VM: heredoc expansion path doesn't resolve array-indexed/array-form expansions, only scalars |
| D-05 | d9.sh | 3 bugs: (1) `for x in "$*"` → sc gives 3 iterations instead of 1 joined; (2) unquoted `$*` → sc no word-split (`ustar:[two three]` vs bash `two`,`three`); (3) `shift` builtin missing entirely ("shift: command not found"), `$#` unchanged | 0/0 | Lowering/VM: `[*]` join; unquoted `$*` splitting; `shift` unimplemented |
| D-06 | d13.sh | `echo "${#f[@]}"` where `f=("${e[@]}")` array-from-array → sc prints `b c 1` (treated as `"${e[@]}"` producing words + count?) — bash prints `3` | 0/0 | IR/lowering: array assignment RHS `"${arr[@]}"` collapses to single string word; `#` length then wrong. (Same class as D-10/D-11) |
| D-07 | d14.sh | `arr=($lines)` with `$lines` = multi-line cmdsub → bash splits into `x y z` (3 elements); sc keeps 1 element with embedded newlines printed as 3 lines. Also `IFS=:` then `echo "${j[*]}"` → bash `a:b:c`, sc `a b c`; `IFS='|'` join also ignored | 0/0 | IR/VM: array literal assignment doesn't word-split unquoted expansion on IFS (incl. newlines); `[*]` ignores current IFS (always space-joins) |
| D-08 | d17.sh | `comb=("${strs[@]}" extra)` where strs=("a b" c) → bash 3 elements, sc 2 (`a b 2`, `c extra`): quoted `"${arr[@]}"` inside array literal not expanded to multiple elements; also earlier `arr=({x,y}...)`/brace pattern in array literal is a PARSE ERROR (see parse divergences) | 0/0 | IR: `"${arr[@]}"` inside array-literal RHS not expanded multi-slot |
| D-09 | d18.sh | `consumer "${res[@]}"` + `local in=("$@")` → sc: `r1: command not found`, `got:0` — passing `"${arr[@]}"` as function args broken; local array from `"$@"` broken (probe confirmed `local b=("$@")` → `x: command not found`) | 0/0 | IR/lowering: `"${arr[@]}"` in call args not expanded per-element; `local arr=("$@")` unsupported |
| D-10 | d19.sh | `g=("${a[@]}")` then `echo "${#g[@]}"` → sc prints `x G:1` (the `x` leaks as a command/word!) and count 1 vs bash 2; `echo "${g[*]}"` → `y` vs `x y` | 0/0 | IR/VM: array-from-quoted-`[@]` broken — value leaks to stdout as word, count wrong |
| D-11 | d12.sh | Glob in array assignment: `g=(dt_tmp/*.txt)` → bash expands to 2 matches; sc keeps literal `dt_tmp/*.txt` (1 element). Glob works in plain command args, fails in array literal RHS | 0/0 | IR: array literal assignment skips glob expansion of elements |
| D-12 | d20.sh | `[[ ${a[1]} == "gamma "* ]]` → bash true, sc false. Isolated probe: `[[ abc == a* ]]` → sc false too; `[[ abc == a* ]] && echo Y || echo NY` prints NY. Glob on RHS of `==` in `[[ ]]` not matched | 0/0 | VM: `BashBinary("==")` compares as string equality, no glob match on RHS (IR dump 0002 PushConst("a*") BashBinary("==")) |
| D-13 | d21.sh | Herestring with unquoted array: `grep -o "h[12]" <<< ${a[@]}` → bash feeds `h1 h2` to grep (prints h1 h2); sc errors `h1: command not found` (word treated as command). Quoted `<<< "${a[@]}"` works in both | 0/0 (sc stdout diff only) | IR/lowering: unquoted `${a[@]}` before `<<<` redirect target not expanded/word-joined |
| D-14 | d22.sh | Scalar overwrite of array: `arr=(a b); arr=scalar; echo "${arr[@]}"` → bash `scalar b` (assigns element 0, keeps rest), sc still `a b` (array untouched) | 0/0 | VM: scalar assignment to array-named var doesn't write arr[0] |
| D-15 | d10.sh (arith) | `$((arr[0] + 1))` → bash `2`, sc `0`. `n++`/`++n` in arithmetic → bash 0/1/2/2, sc all 0 (increment ops no-op). Plain arith `x+1` etc. OK | 0/0 | IR/VM: array element in arithmetic evaluates to 0; `++`/`--` postfix/prefix have no effect (d10.sh kept as divergence script) |

Note: d10 outputs saved in `difftest/div-d/d10.{bash,sc}.out`.

## Parse-time divergences (compiler error — script won't compile at all)

| # | Construct | Error | Expected (bash) |
|---|-----------|-------|-----------------|
| P-01 | `echo "${arr[0]:-fallback}"` / any `${arr[i]:-def}` / `${arr[@]:1}` slice | `parse error: expected '}' after array index` | default/slice ops on array element |
| P-02 | `arr[$i]=v` (variable index in assignment LHS) | parsed but runs as command: `arr[2]=TWO: command not found` — actually parse/lowering accepts only literal digit index | assigns arr[2]=TWO (d7 original) |
| P-03 | `${arr[$((1+2))]}` | VM error `bad array index '$((1+2))'` — literal `$((...))` not evaluated in index | reads arr[3] |
| P-04 | `mixed=({x,y}{1,2})`, `arr=(a{1..3})`, `z=({a..c})` — brace pattern in array literal | `parse error: unsupported token in array literal` | brace expansion to multiple elements (works in scalar context `x{a,b}`... see D-16) |
| P-05 | `${var: -2}` (space before negative offset) | `not implemented: variable expansion operator` | last 2 chars (d11) |
| P-06 | case pattern mixing quotes+glob: `"g"*d*)` | `unexpected token 'expected ')', got '*d*''` | bash pattern g…d match |
| P-07 | `shift` builtin | runtime `shift: command not found` (also in D-05) | shifts $1.. |

## Additional runtime notes (not separate scripts)

- `echo "$arr"` where arr is array → empty in sc (D-01 class, probe confirmed)
- Heredoc `<<EOF` containing only array refs → all empty (D-04 class; scalar refs fine)
- `for x in ${files[@]}` unquoted array in for-list → sc correct (no splitting, elements already atomic) — passes
- Arrays in subshell/pipe/cmdsub of atomic elements → pass (d5)
- unset arr[i], sparse, ${#arr[@]} after sparse unset, arr[100]=x grow → pass (d3)
- ${VAR#pat} with `*`, `%%.*`, substring, :- := :+ on scalars → pass (d11)
- loops with "${arr[@]}", break/continue, case on "${arr[0]}" → pass (d16, d20 partial)
- printf '%s\n' "${res[@]}" multi-arg, cmdsub `$(echo "${arr[@]}")` → pass (d18, d19 partial)

## Scripts created this round

d1–d22 (22 scripts). PASS: d3, d5, d6, d7, d11, d16 (6 pass; .out files deleted, .sh kept as regression).
DIVERGE (kept .out in difftest/div-d/): d1, d2, d4, d8, d9, d10, d12, d13, d14, d17, d18, d19, d20, d21, d22 (15).
d10: arithmetic divergence — `$((arr[0]))` → 0, `n++`/`++n` no-op (see D-15).

---

## Resolution log (2026-09-07, fix session)

All divergences fixed; full suite d1–d22 + c1–c22 + arr1–3 passes
(only c15b error-prefix byte diff remains — accepted, script header documents it).

- D-01..D-15: fixed in shell_vm (env.rs, vm.rs), shell_ir (lowering.rs).
- P-01 `${arr[i]:-def}`: new `VarExpandOp::IndexDefault` (op_byte 18).
- P-02 `arr[$i]=v`: LHS flatten+parse in `try_parse_assignment`.
- P-03 `${arr[$((expr))]}`: `subst_arith_vars` + ArithParser in eval_array_index.
- P-04 brace in array literal: `try_brace_expand_in_elems` + adjacent-pair
  folding via `expand_adjacent_braces` (also fixed `echo {x,y}{1,2}`).
- P-05 `${var: -2}`: space-before-negative-offset in parse_brace.
- P-06 `"g"*d*)`: adjacent-fragment merge in case patterns.
- P-07 `shift`: BuiltinId::Shift (0x14).
- local arr=(…): parser local_arrays → ArrayAssign local bit 30; env
  set_array_local/array_append_local.
- trap body now compiles+runs in a child VM (was `sh -c` — lost arrays).

---

# Divergences — difftest round "d" (edge-case regression, 2026-09-07)

Date: 2026-09-07 (second pass). Compiler: `./target/release/shellsc build difftest/dN.sh -o difftest/dN.sc`.
Run: `bash difftest/dN.sh` vs `env -u LD_PRELOAD ./difftest/dN.sc`.
Scripts: d23–d42 (20 scripts). Focus: function redirects + arrays, empty/sparse arrays, local/global interaction, array-from-array, IndexDefault, trap+array, herestring+array, quoted vs unquoted `[@]`, redirect chains, case patterns with arrays, arithmetic indices.

## Results summary

| # | Script | Result | Notes |
|---|--------|--------|-------|
| d23 | Function redirect + array inside function | PASS | `} 2>/dev/null` applied correctly across multiple calls |
| d24 | Function redirect suppressing stderr, caller override | PASS | |
| d25 | Empty array edge cases | **FAIL** | `for x in "${arr[@]}"` on empty array: bash 0 iterations, sc 1 iteration (empty string) |
| d26 | Sparse array indices | PASS | |
| d27 | Local array vs global array interaction | PASS | |
| d28 | Array-from-array with modifications | PASS | |
| d29 | IndexDefault on arrays | PASS | `${arr[i]:-word}` works |
| d30 | Trap + array interaction | PASS | Trap body compiled in child VM sees arrays |
| d31 | Herestring with arrays (quoted [@], [*], [0]) | PASS | |
| d32 | Quoted vs unquoted ${arr[@]} in for loops | PASS | |
| d33 | Nested function calls with arrays and redirects | PASS | `local iarr=("$@")` works |
| d34 | Array append += single and multiple | PASS | |
| d35 | Redirect chain on compound command `{ } 2>/dev/null \| cat` | **FAIL** | stderr redirect not applied before pipe; `err1` leaks through in sc |
| d36 | Array expansion in case pattern | **FAIL** | `"${arr[1]}"` matches arr[0] instead of arr[1] — wrong element |
| d37 | Function redirect + nested call + array passing | PASS | |
| d38 | Array index arithmetic | **FAIL** | `${arr[i+1]}` bare expression: "bad array index 'i+1'" (no arithmetic eval) |
| d39 | Local array shadowing global | PASS | |
| d40 | Herestring with array and command substitution | PASS | |
| d41 | Trap body modifying arrays | PASS | |
| d42 | Complex: func redirect + local array + array-from-array + herestring | PASS | |

**PASS: 16, FAIL: 4**

## Runtime divergences (detail)

### D-NEW-01 (d25): Empty array `for` iteration

```bash
arr=()
for x in "${arr[@]}"; do echo "iter:$x"; done
```

- bash: 0 iterations (loop body never executes)
- sc: 1 iteration with empty string (`iter:` printed)
- Exit: bash=0, sc=0
- Diagnosis: VM/IR: `"${arr[@]}"` on empty array produces one empty-string slot instead of zero slots. The `for` loop then iterates once over that empty string.

### D-NEW-02 (d35): Redirect chain on braced compound command piped

```bash
{ echo "line1"; echo "line2"; echo "err1" >&2; } 2>/dev/null | cat
```

- bash: `line1\nline2` (stderr suppressed by `2>/dev/null` before pipe)
- sc: `err1\nline1\nline2` (stderr leaks — redirect not applied to the compound block before piping)
- Exit: bash=0, sc=0
- Diagnosis: IR/lowering: when a braced group has both a redirect and a pipe, the redirect is not applied to the group's fd table before the pipe is set up. The `2>/dev/null` should suppress fd2 output from reaching the terminal, but it appears to be ignored or applied after the pipe captures everything.

### D-NEW-03 (d36): Case pattern with array element expansion

```bash
arr=(foo bar)
val="bar"
case "$val" in
  "${arr[0]}") echo "match first" ;;
  "${arr[1]}") echo "match second" ;;
esac
```

- bash: `match second` (correctly expands `${arr[1]}` to `bar`)
- sc: `match first` (appears to expand `${arr[1]}` as `foo`, matching the first pattern)
- Exit: bash=0, sc=0
- Diagnosis: IR/lowering: array index in case pattern expansion may be using wrong index or always resolving to element 0. Need to check how `${arr[N]}` is lowered inside case pattern context.

### D-NEW-04 (d38): Bare arithmetic expression in array index

```bash
arr=(zero one two three four)
i=2
echo "${arr[i+1]}"
```

- bash: `three` (evaluates `i+1` as arithmetic → 3)
- sc: `running bytecode: bytecode error: bad array index 'i+1'`
- Exit: bash=0, sc=1
- Diagnosis: VM: `eval_array_index` handles `$((expr))` and `$var` forms but does not evaluate bare arithmetic expressions like `i+1` without the `$((...))` wrapper. Bash treats any array subscript as an arithmetic context automatically.

## Scripts created this round

d23–d42 (20 scripts). PASS: d23, d24, d26, d27, d28, d29, d30, d31, d32, d33, d34, d37, d39, d40, d41, d42 (16). FAIL: d25, d35, d36, d38 (4).
All .sh files kept as regression tests. All .sc and temp output files cleaned up.

## Round f — ANSI-C quoting, heredoc params, bg functions, read -a, stdin cursor

Scripts f1–f48 (batch tests plus minimal isolations). Clean PASS: f5, f8, f10, f11, f14, f17, f22, f28, f33, f36, f39, f41, f45, f46. DIVERGE: f1, f2, f3, f4, f6, f7, f9, f12, f13, f15, f16, f18, f19, f20, f21, f23, f24, f25, f26, f27, f29, f30, f31, f34, f35, f37, f38, f40, f42, f43, f44, f47, f48 (some of these batch scripts contain multiple divergences, each isolated below).

### D-F-01 (f1, f31): Default $IFS missing tab

    printf '%s\n' "$IFS" | od -c   # fresh run, no prior assignment

- bash: " \t \n" (space, tab, newline)
- sc: " \n" (space, newline — no tab)
- Diagnosis: crates/shell_vm — VM-level default IFS constant is " \n" not " \t\n". Note read.rs lines 74/87 default " \t\n" are correct; only the general env default differs. Affects all field splitting until user assigns IFS.

### D-F-02 (f2, f3, f34): Panic on NUL value from \0 / octal \0

    c=$'\0'
    printf 'len:%s\n' "${#c}"

- bash: len:0
- sc: thread 'main' panicked at env.rs:362: failed to set environment variable "c" to "\0": file name contained an unexpected NUL byte — whole process aborts
- Diagnosis: shell_vm SetVar or export path calls std::env::set_var with NUL in value. Shell semantics drop NUL; must truncate or ignore, never panic.

### D-F-03 (f21, f27): \cX control escapes not decoded

    printf '%s' $'\ca' | od -An -c   # want 001
    printf '%s' $'\cA' | od -An -c   # want 001
    printf '%s' $'\c[' | od -An -c   # want 033 (ESC)

- sc emits literal backslash-c-X ( \ c a ) for all three
- Diagnosis: crates/shell_lex/src/lexer.rs read_ansi_c_body — \c control form unimplemented. Bash: \cX = X & 0x1F.

### D-F-04 (f6, f29, f42): \' inside $'...' dropped, not emitted

    printf '%s' $'a\'b' | od -An -c   # bash: a ' b ; sc: a b
    printf '%s' $'it\'s' | od -An -c  # bash: i t ' s ; sc: i t s

- Also $'a\'b'c mid-word (f42).
- Diagnosis: crates/shell_lex/src/lexer.rs read_ansi_c_body — \' branch consumes both chars but emits nothing (likely stops scan as if string terminated).

### D-F-05 (f7): Heredoc with $'...' delimiter — build fails

    read -r line <<$'END\tX'
    hello
    $'END\tX'
    echo "line:$line"

- bash: warns unterminated, prints line:$'END\tX'
- sc: Error: lex error at 11:1: unterminated heredoc for delimiter 'END\tX' — build fails
- Diagnosis: crates/shell_lex/src/lexer.rs heredoc delimiter scan does not understand $'...' as delimiter word (bash: quoted delimiter, no expansion, tab is literal part of delimiter).

### D-F-06 (f15, f24): <<$'EOF' delimiter treated as EXPANDING heredoc

    cat <<$'END'
    literal $1 $(x) $HOME
    END

- bash: literal $1 $(x) $HOME ($'...' delimiter ⇒ quoted heredoc, no expansion)
- sc: literal  $(x) /home/... ($HOME expanded, $1 empty)
- Diagnosis: crates/shell_lex/src/lexer.rs delimiter classification — $'...' must set the quoted/no-expand heredoc flag like '...' does.

### D-F-07 (f9): $0 inside heredoc

- bash: zero=difftest/f9.sh, sc: zero=./f9.sc — expected $0 difference, NOT a bug. Excluded.

### D-F-08 (f12, f48): read without -r keeps backslashes literal

    printf 'a\\tb\n' | { read -a arr; echo "0:[${arr[0]}]"; }    # bash: [atb], sc: [a\tb]
    printf 'x\\ y\n' | { read -a arr; echo "0:[${arr[0]}]:1:[${arr[1]}]"; }  # bash: [x y]:[], sc: [x\]:[y]

- Diagnosis: crates/shell_vm/src/builtins/read.rs — -r flag parsed but unused; no backslash-escape processing exists for non-raw read.

### D-F-09 (f13, f30): read does not trim leading/trailing IFS whitespace

    read -r z <<< "  spaces  "     # bash: [spaces], sc: [  spaces  ]
    read -r p q <<< "  a  b  "    # bash: p:[a] q:[b], sc: p:[] q:[a  b  ]

- Diagnosis: crates/shell_vm/src/builtins/read.rs line 93 — only trim_start_matches on splitn fields; single-var whole-line read never trimmed; no trailing strip at all.

### D-F-10 (f13, f25, f40): while read from < file processes only first line

    printf 'A\nB\nC\n' > f40in.txt
    while read -r l; do
      echo "got:$l"
    done < f40in.txt

- bash: got:A got:B got:C ; sc: got:A only
- Diagnosis: crates/shell_vm/src/vm.rs Read builtin / loop stdin: file-redirect stdin reopened or cursor reset each iteration, or remainder discarded after first read. Piped while-read (f41) and here-string while-read (f33) PASS — only < file form broken.

### D-F-11 (f15, f20): $( ) command substitution in unquoted heredoc body not expanded

    cat <<EOF
    expand:$(echo inline)
    EOF

- bash: expand:inline ; sc: expand:$(echo inline)
- Diagnosis: crates/shell_vm/src/vm.rs expand_heredoc_body — handles $1..$9 $# $* $@ $? $$ $0 but not command substitution (and see D-F-16 for ${} forms). Same construct at top level (f28) works; gap is heredoc-body-expander-specific.

### D-F-12 (f16, f26, f47): bg function 2> redirect lost, stderr leaks to parent

    o() { echo "O"; echo "E" >&2; }
    o > f47o.txt 2> f47e.txt &
    wait

- bash: f47o.txt=O, f47e.txt=E, console clean
- sc: E printed to console, f47e.txt never created, result o:[O]e:[]
- Diagnosis: crates/shell_vm/src/vm.rs Opcode::ExecExternalBg child — only stdout redirect transferred; stderr/fd2 redirects dropped.

### D-F-13 (f38): two bg calls in one list — second loses function

    p() { echo "pl"; }
    p & p &
    wait

- bash: pl twice ; sc: first OK, second → shellsc: p: command not found
- Diagnosis: crates/shell_vm/src/vm.rs ExecExternalBg — pending-bg state clobbered after first spawn in same AND-OR list. Two separate lines (f39) PASS.

### D-F-14 (f19, f23, f35): unquoted case subject with IFS char splits

    v=$'x\ty'
    case $v in
      *$'\t'*) echo M1 ;;
      *) echo N1 ;;
    esac

- bash: M1 ; sc: "x N1" (subject split at tab: x matched * arm; leftover field pollutes echo args)
- Diagnosis: crates/shell_vm — GlobExpand on case subject does field splitting; CaseBegin pops one field, leftovers corrupt stack. IR shows PushVar("v") GlobExpand CaseBegin. Lowering should suppress split for case subject or VM consume all fields.

### D-F-15 (f44): ${N:-default} positional with modifier — parse error

    set -- A B
    echo "${1:-D}"

- bash: A ; sc: Error: parse error at 3:6: expected '}' — BUILD FAILS
- Diagnosis: crates/shell_parse/src/parser.rs is_special_var includes digits; after consuming special char the parser demands immediate '}' — no modifier path for positionals (${1:?} ${1:+} ${1-D} ${3:+SET} all fail). Named ${x:-D} works.

### D-F-16 (f37, f43): ${1:-default} inside heredoc expands empty

    f() { cat <<EOF
    d:${1:-DEFAULT}
    EOF
    }
    f X    # bash: d:X, sc: d:

- Diagnosis: crates/shell_vm/src/vm.rs expand_heredoc_body — brace param-expansion forms (${...}) not implemented in heredoc expander; only bare $1-style.

### D-F-17 (f18, f32): <(...) process substitution in redirect — parse error

    read b < <(echo second)

- bash: b=second ; sc: Error: unexpected token '<' at 3:10 — build fails
- Diagnosis: crates/shell_parse/src/parser.rs — process substitution unsupported in redirect position. Pre-existing gap surfaced by stdin-cursor tests. Rest of f18 (here-string cursor chains) passes when procsub line removed.

### Verified OK (no divergence)

- ANSI-C: \t \n \r \a \b \e \E \f \v \\ \" \? octal \0NNN (incl \1011 → A1), hex \xHH \x7f, empty $'', unknown escapes \q \z literal, concat pre$'x'post, "$var"$'\n', as args, IFS=$'...' assignment, for-loop items, printf $'fmt' (f5 f14 f22 f36)
- Heredoc positional params: $1..$9 $# $* $@ $? in unquoted heredoc; quoted <<'EOF' correctly NOT expanding; heredoc in function with locals; after set --; here-strings with $1 (f8, f9 minus $0)
- bg functions: args myfn a b &, file writes, stdout > redirect, subshell var isolation, bg external sleep 0.05 &, bg fn in pipeline, bg fn with heredoc inside (f10 f11 f45 f46; f38 partial)
- read -a: -r -a, combined -ra, IFS variants (: and tab), piped stdin, heredoc redirect stdin, empty line, multiple/leading/trailing spaces in -a mode (f12 minus backslash, f17)
- stdin cursor: consecutive here-strings read x <<< v1; read y <<< v2, while-read with here-string, piped while-read (f13 partial, f33, f41)

## Round f scripts

f1–f48 kept as .sh regression tests. .sc and output .txt files removed after recording.

## g-series difftest (2026-09-10)

| Test | Issue | Status |
|------|-------|--------|
| g4 | Recursive function with cmdsub `$(fib $(($1-1)))` returns wrong value (sc=4, bash=21) | Known bug - nested cmdsub in recursive fn |
| g10 | `exec 3>&1` + `echo >&3` + `exec 3>&-` — output lost on fd 3 | Known bug - exec fd redirect write |
| g15 | `(exit 42) &` runs inline instead of background — lowering ignores `&` for subshells | Known limitation - subshell bg not implemented |
| g16 | `printf 'hello\\' | read -r x` — bash: x="" rc=1 (EOF no newline), sc: x="hello\" rc=0 | Known bug - read from pipe EOF handling |
