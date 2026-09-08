# "$@" must split into N positional words, one loop iteration each
# DIVERGENT: bash iterates a / b c / d; ShellSC iterates once with "a b c d".
# Suspected stage: shell_ir/shell_vm expansion (quoted $@ collapses to single word).
# 2026-09-04
set -- a "b c" d
for w in "$@"; do printf 'E:[%s]\n' "$w"; done
