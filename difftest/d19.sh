# quoted array in command substitution + nested quoting
a=(x y)
echo "A:$(echo "${a[@]}")"
echo "B:[$(echo "${a[0]}${a[1]}")]"
v=$(printf '%s,' "${a[@]}")
echo "C:$v"
w=$(for e in "${a[@]}"; do echo "$e"; done)
echo "D:$w"
echo "E:$(echo "in ${a[@]} mid")"
f() { echo "${a[@]}"; }
echo "F:$(f)"
g=("${a[@]}")
echo "G:${#g[@]}"
echo "H:${g[*]}"
