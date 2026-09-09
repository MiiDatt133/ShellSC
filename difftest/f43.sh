# f43: bg function reading var — heredoc params in bg fn (f37 ${1:-} variant)
b() {
  cat <<EOF
d:${1:-DEFAULT}
n:${#2}
EOF
}
b X
b
