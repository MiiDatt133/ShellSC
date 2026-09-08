fruits=(apple banana cherry)
echo "quoted: ${fruits[@]}"
echo "unquoted: ${fruits[@]}"
echo "star: ${fruits[*]}"
echo "count: ${#fruits[@]}"
for f in "${fruits[@]}"; do echo "each:$f"; done
printf '%s\n' "${fruits[@]}"
i=1
echo "varidx: ${fruits[$i]}"
last=${#fruits[@]}
arr2=([0]=zero)
