# f38: bg fn with heredoc inside + bg inside pipeline
b() { cat <<EOF
bg-hd:$1
EOF
}
b BGARG &
wait
p() { echo "pl"; }
p | cat
p & p &
wait
echo "two-bg-done"
