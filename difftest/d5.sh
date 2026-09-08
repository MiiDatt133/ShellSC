# subshell, command substitution, pipe with arrays
a=(1 2 3)
(
  echo "sub:${a[@]}"
  a[0]=9
  echo "sub2:${a[@]}"
)
echo "parent:${a[@]}"
v=$(echo "${a[1]}")
echo "cs:$v"
w=$(echo ${a[@]})
echo "cs2:[$w]"
echo "${a[@]}" | while read x; do echo "p:$x"; done
b=(${a[@]} 4)
echo "${b[@]}"
c=("${a[@]}" 4)
echo "${c[*]}"
