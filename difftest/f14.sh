# f14: ANSI-C quoting in assignments + arithmetic + various
TAB=$'\t'
NL=$'
'
printf 'tab:%s:nl:%s:\n' "$TAB" "$NL" | od -c | head -3
A=$'\t'; B='x'
echo "$B$A$B" | od -c | head -1
C=$'a\\b'
printf '%s' "$C" | od -c
D=$'\x41\101'
echo "$D"
IFS=$':' read -r f1 f2 <<< "u:v"
echo "f=$f1:$f2"
echo "multi$NL""line" | od -c | head -2
