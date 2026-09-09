# f17: read -a edge: IFS tab/space, trailing, quotes
IFS=$' \t'
printf 'a\tb  c\n' | { read -a arr; echo "n:${#arr[@]}:${arr[0]}:${arr[1]}:${arr[2]}"; }
printf 'a\tb  c\n' | { read -a arr; echo "2:${arr[1]}"; }
printf ' a\tb \n' | { read -a arr; echo "trim:${#arr[@]}:${arr[0]}:${arr[1]}"; }
IFS=$'\t' read -a t <<EOF
p	q	r
EOF
echo "tabifs:${#t[@]}:${t[0]}:${t[1]}:${t[2]}"
printf 'a\nb\n' | { read -a arr1; read -a arr2; echo "seq:${arr1[0]}:${arr2[0]}"; }
