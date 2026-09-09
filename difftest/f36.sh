# f36: ANSI-C in loop read, IFS tests, command args
IFS=$'\n'
printf 'a\tb\n' | { read -r l; echo "nlIFS:[$l]"; }
IFS=$' \t\n'
echo "$IFS" | od -An -c
for x in $'p\tq' $'r'; do echo "item:[$x]"; done
printf 'x\tb\n' | while IFS=$'\t' read -r a b; do echo "tb:[$a][$b]"; done
