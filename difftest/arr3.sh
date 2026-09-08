# sparse index
sp[5]=five
echo "sp0:[$sp[0]]" 2>/dev/null || true
echo "sp5:[${sp[5]}]"
sp[0]=zero
echo "len:${#sp[@]}"
# empty literal
e=()
echo "elen:${#e[@]}"
# append multiple
m=(a b)
m+=(c d e)
echo "m: ${m[@]} len:${#m[@]}"
# function visibility
setit() { arr=(x y); }
setit
echo "after: ${arr[1]}"
# overwrite
w=(1 2 3)
w=(9)
echo "w: ${w[@]} len:${#w[@]}"
# quoted string with spaces as one element
q=("hello world" second)
echo "q1:[${q[0]}]"
for x in "${q[@]}"; do echo "q:$x"; done
