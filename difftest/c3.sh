i=0
while true; do
  i=$((i+1))
  if [ "$i" -eq 3 ]; then break; fi
  if [ "$i" -eq 1 ]; then continue; fi
  echo "i=$i"
done
for x in 1 2 3 4 5; do
  [ "$x" -eq 3 ] && continue
  [ "$x" -eq 5 ] && break
  echo "x=$x"
done
