# f28: isolated — nested heredoc inside command substitution (from f20)
set -- P1
echo "outer-sub"
r=$(cat <<INNER
inner-$1
INNER
)
echo "got:$r"
cat <<OUTER
outer-$1
OUTER
echo "done"
