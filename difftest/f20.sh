# f20: heredoc params in bg functions + nested heredocs
h() {
  cat <<EOF
hf:$1:$#:star=$*
EOF
}
h M N &
wait
set -- S1 S2
cat <<EOF
post-set:$1:$2:$#:$?
EOF
cat <<OUTER
outer-$1
$(cat <<INNER
inner-$1
INNER
)
OUTER
echo "seq:$(echo "$1")"
read -r q <<< "$1 tail"
echo "q=$q"
