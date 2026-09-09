# f41: isolated — piped while read loop multi-line
printf 'L1\nL2\nL3\n' | while read -r l; do
  echo "p:$l"
done
echo "after"
