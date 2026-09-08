# arrays basic: unquoted ${arr[@]} word-splitting
arr=(a b c)
echo ${arr[@]}
echo "$arr"
echo "${arr[0]}"
echo ${#arr[@]}
echo "pre${arr[1]}post"
arr[1]=Z
echo "${arr[@]}"
arr+=(d e)
echo "${arr[@]}" "${#arr[@]}"
exit 0
