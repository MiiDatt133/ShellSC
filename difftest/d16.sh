# while/until loop over array indices; break/continue
arr=(10 20 30 40)
i=0
while [ "$i" -lt "${#arr[@]}" ]; do
  echo "w:$i=${arr[$i]}"
  i=$((i+1))
done
for v in "${arr[@]}"; do
  if [ "$v" = "20" ]; then continue; fi
  if [ "$v" = "40" ]; then break; fi
  echo "v:$v"
done
n=0
until [ "$n" -ge 3 ]; do
  echo "u:$n"
  n=$((n+1))
done
for idx in 0 1 2; do echo "idx:${arr[$idx]}"; done
