# f37: heredoc params deep: $@ inside quotes, ${1:-default}, nested funcs
f() {
  cat <<EOF
q:"$@" uq:$@ st:"$*" sq:'$1' dq:"$1"
def:${1:-DEFVAL}
EOF
}
f a b c
g() { f inner1 inner2; }
g
set --
cat <<EOF
empty:#=$#:star:$*:at:$@
EOF
