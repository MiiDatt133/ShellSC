# f42: isolated — ANSI-C with \c in middle of word + concatenation patterns
echo a$'\t'b$'\n'c | od -An -c
x=$'one'two
echo "$x"
y=$'A'"B"$'C'
echo "$y"
echo $'a\'b'c | od -An -c
