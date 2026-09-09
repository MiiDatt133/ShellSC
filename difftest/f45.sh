# f45: isolated — bg fn write to same append file, sequential order
w() { echo "line:$1"; }
rm -f f45.txt
for i in 1 2 3; do
  w $i >> f45.txt &
  wait
done
echo "content:"
cat f45.txt
rm -f f45.txt
