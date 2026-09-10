arr=(a b c d)
unset arr[1]
echo "${arr[@]}"
echo "${#arr[@]}"
