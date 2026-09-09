# f5: ANSI-C in words, concat, args, case patterns
echo pre$'x'post
v=mid
echo a"$v"$'\t'b
echo $'arg1\targ2' | cat
case $'ab' in
  $'a'*) echo "case-match-ok" ;;
  *) echo "case-no" ;;
esac
case abc in
  a$'b'c) echo "case2-ok" ;;
  *) echo "case2-no" ;;
esac
printf '%s\n' $'one' $'two'
set -- $'a\tb' $'c'
echo "argc:$#:1:$1:2:$2"
