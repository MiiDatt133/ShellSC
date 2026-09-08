# arr[$var] read/write; literal $((...)) index unsupported — record separately
arr=(a b c d)
v=X
arr[1]=$v
echo "${arr[@]}"
i=3
echo "${arr[$i]}"
