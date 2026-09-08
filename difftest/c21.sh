# heredoc with quoted delimiter <<'EOF' must be literal (no expansion)
# DIVERGENT: bash prints "hi $name" literally; ShellSC expands to "hi world".
# Suspected stage: shell_parse/shell_ir heredoc quoting flag not honored.
# 2026-09-04
name=world
cat <<'EOF'
hi $name
EOF
