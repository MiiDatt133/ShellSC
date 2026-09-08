# arrays with plain elements + concat (no brace patterns in array literals)
nums=(1 2 3)
strs=("a b" c)
comb=("${strs[@]}" extra)
echo "${#comb[@]}"
echo "${comb[@]}"
echo "n:${#nums[@]} s:${#strs[@]}"
pre="p"
echo "x${strs[0]}y"
