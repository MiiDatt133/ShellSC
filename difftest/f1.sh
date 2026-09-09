# f1: ANSI-C quoting basic escapes
s1=$'a\tb\n'
printf '%s' "$s1" | od -c | head -2
echo "len:${#s1}"
printf '%s\n' "$IFS" | od -c | head -1
IFS=$' \t\n'
printf '%s\n' "$IFS" | od -c | head -1
echo "c$'a'd" 2>/dev/null || echo "concat fail"
