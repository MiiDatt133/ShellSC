# f13: consecutive reads with here-strings
read x <<< "v1"
read y <<< "v2"
echo "x=$x y=$y"
i=0
while read -r line; do
  echo "loop:$line"
  i=$((i+1))
done <<< "L1
L2"
echo "count=$i"
printf 'F1\nF2\n' > f13in.txt
while read -r l2; do
  echo "file:$l2"
done < f13in.txt
rm -f f13in.txt
echo "a|b" | while read -r p q; do
  echo "pipe:$p/$q"
done
read -r a1 b1 c1 <<< "one two three four"
echo "hs:$a1:$b1:$c1"
read -r z <<< "  spaces  "
echo "trim:[$z]"
