# redirection edge cases
echo "both" > out8.txt 2>&1
cat out8.txt
echo "stderr only" 1>&2
noline() { echo no-newline; }
if command -v printf >/dev/null; then
  printf "%s" "no newline"
  echo " after"
fi
exec 3< out8.txt 2>/dev/null || echo "no fd3"
