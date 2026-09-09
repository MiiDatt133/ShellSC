# f3: octal \0NNN and hex \xHH
a=$'\101\102\103'
printf '%s\n' "$a"
b=$'\x41\x42\x43'
printf '%s\n' "$b"
c=$'\0'
printf 'oct0:%s:\n' "$c" | od -c | head -1
d=$'\1011'
printf '%s\n' "$d" | od -c | head -1
e=$'\x7f'
printf '%s' "$e" | od -An -tx1
