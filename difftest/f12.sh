# f12: read -a variants
echo "a b c" | { read -a arr; echo "n:${#arr[@]}:0:${arr[0]}:2:${arr[2]}"; }
echo "a b c" | { read -r -a arr; echo "n:${#arr[@]}:0:${arr[0]}"; }
echo "a b c" | { read -ra arr; echo "n:${#arr[@]}:0:${arr[0]}:1:${arr[1]}"; }
echo " a  bb   ccc " | { read -a arr; echo "n:${#arr[@]}:0:${arr[0]}:1:${arr[1]}:2:${arr[2]}"; }
IFS=: read -a arr <<EOF
x:y:z
EOF
echo "ifs:n:${#arr[@]}:0:${arr[0]}:1:${arr[1]}:2:${arr[2]}"
echo "one" | { read -a arr; echo "single:n:${#arr[@]}:0:${arr[0]}"; }
echo "" | { read -a arr; echo "empty:rc=$?:n:${#arr[@]}"; }
printf 'r\\tb\n' | { read -a arr; echo "backslash:r0:${arr[0]}:r1:${arr[1]}"; }
printf 'r\\tb\n' | { read -r -a arr; echo "raw:r0:${arr[0]}:r1:${arr[1]}"; }
