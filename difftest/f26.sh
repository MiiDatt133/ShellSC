# f26: isolated — bg function redirect combos (from f16)
w() { echo "w1:$1"; echo "w2:"; }
rm -f a26.txt
w Z >> a26.txt &
wait
echo "app1:$(cat a26.txt)"
rm -f a26.txt
o() { echo "out"; echo "err" >&2; }
o > b26_out.txt 2> b26_err.txt &
wait
echo "o:$(cat b26_out.txt):e:$(cat b26_err.txt)"
rm -f b26_out.txt b26_err.txt
