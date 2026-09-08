# fd ordering: 2>&1 1>file vs 1>file 2>&1
# CLEAN: both impls match. First form sends stderr to old stdout (tty/pipe), second captures both.
# 2026-09-04
printf 'A-out\n'
printf 'A-err\n' >&2
printf 'B-out\n' 1>c22_b.txt 2>&1
printf 'C-err\n' 2>&1 1>c22_c.txt
cat c22_b.txt c22_c.txt
