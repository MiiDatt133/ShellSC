i=0
while [ $i -lt 2 ]; do
cat <<LOOP
iter=$i
LOOP
i=$((i+1))
done
