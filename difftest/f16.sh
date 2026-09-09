# f16: bg function args + redirect combos
w() { echo "w1:$1"; echo "w2:$2"; }
w X Y &
wait
w Z >> bgappend.txt &
w W >> bgappend.txt &
wait
echo "app:$(cat bgappend.txt)"
rm -f bgappend.txt
e() { return 3; }
e &
wait
echo "rc=$?"
o() { echo "to-stderr" >&2; echo "to-stdout"; }
o > bgboth_out.txt 2> bgboth_err.txt &
wait
echo "out:$(cat bgboth_out.txt) err:$(cat bgboth_err.txt)"
rm -f bgboth_out.txt bgboth_err.txt
