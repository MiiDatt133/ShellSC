# f19: ANSI-C in arithmetic, test args, printf %b comparison
t=$'\t'
[ "$t" = "$(printf '\t')" ] && echo "tab-eq"
[ $'\x41' = A ] && echo "hex-A"
echo $'a\tb' | grep -c $'\t' || true
n=$'\n'
[ -z "$n" ] && echo "empty" || echo "nonempty"
v=$'x\ty'
case $v in
  *$'\t'*) echo "tab-in-case" ;;
esac
echo "arith:$(( 1 + 2 ))"
printf '%b' 'a\tb\n' | od -c | head -1
printf '%s' $'a\tb\n' | od -c | head -1
