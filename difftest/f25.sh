# f25: isolated — while read from file redirect
printf 'F1\nF2\n' > f25in.txt
while read -r l; do
  echo "file:$l"
done < f25in.txt
echo "after-loop"
rm -f f25in.txt
