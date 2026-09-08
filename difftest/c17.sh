# echo -e flag and backslash escapes
# DIVERGENT: bash prints "a<TAB>b" (interprets -e); ShellSC prints "-e a\tb" (flag not stripped, no escape interpretation). exit 0 both.
# Suspected stage: shell_ir builtin echo (option parsing / escape handling).
# 2026-09-04
echo -e 'a\tb'
