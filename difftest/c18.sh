# printf format-arg reuse: extra args re-run format
# DIVERGENT: bash prints a/b/c on 3 lines; ShellSC prints only "a". exit 0 both.
# Suspected stage: shell_ir/shell_vm printf builtin (no format reuse loop).
# 2026-09-04
printf '%s\n' a b c
