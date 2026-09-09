# f34: isolated — NUL-containing assignment from $'\0' — env var NUL panic (f2/f3)
c=$'\0'
printf 'len:%s\n' "${#c}"
