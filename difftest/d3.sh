# unset arr[i], sparse, ${#arr[@]} after unset
arr=(a b c d)
unset arr[1]
echo "${#arr[@]}"
echo "${arr[0]}-${arr[1]}-${arr[2]}-${arr[3]}"
for i in "${arr[@]}"; do echo "v=[$i]"; done
unset arr[0]
echo "${#arr[@]}"
arr+=(X)
echo "${#arr[@]} ${arr[2]} ${arr[4]}"
echo "${arr[@]}"
