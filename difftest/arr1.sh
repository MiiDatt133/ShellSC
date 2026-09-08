arr=(a b c)
echo "${arr[0]}${arr[2]}"
echo "${arr[@]}"
echo "${#arr[@]}"
arr[1]=X
echo "${arr[1]}"
arr+=(d)
echo "${arr[3]}" "${#arr[@]}"
unset arr[1]
echo "len=${#arr[@]}"
for x in "${arr[@]}"; do echo "item:$x"; done
