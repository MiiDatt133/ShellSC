# f40: isolated — while read loop from file, multi-line
printf 'A\nB\nC\n' > f40in.txt
while read -r l; do
  echo "got:$l"
done < f40in.txt
rm -f f40in.txt
