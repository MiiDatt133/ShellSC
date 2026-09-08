# ${v-def} colon-less default (unset => def; empty stays empty)
# HARD DIVERGENT: bash prints def; ShellSC build fails: parse error at 1:6: expected '}' or operator in ${}
# (${v-def}, ${v+alt}, ${v=alt} all rejected; only :- :+ := accepted)
# Suspected stage: shell_parse parameter-expansion grammar.
# 2026-09-04
echo "${uv-def}"
