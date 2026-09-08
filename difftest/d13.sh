# empty array + unset whole array + reappend
e=()
echo "n=${#e[@]}"
e+=(a)
echo "n=${#e[@]} v=${e[@]}"
unset e
echo "n=${#e[@]}"
e+=(b c)
echo "n=${#e[@]} v=${e[@]}"
unset u
u+=(first)
echo "n=${#u[@]} v=${u[@]}"
e[5]=five
echo "${#e[@]}"
echo "${e[5]}"
echo "${e[@]}"
f=("${e[@]}")
echo "${#f[@]}"
g=(${e[@]})
echo "${#g[@]}"
