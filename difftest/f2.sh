# f2: ANSI-C full escape set
a=$'\a\b\e\E\f\v\r\0'
printf '%s' "$a" | od -c
b=$'\\'
printf '%s' "$b" | od -c
c=$'\''
printf '%s' "$c" | od -c
d=$'\"'
printf '%s' "$d" | od -c
e=$'\?'
printf '%s' "$e" | od -c
