# array via command substitution output, IFS join
lines=$(printf 'x\ny\nz\n')
arr=($lines)
echo "${#arr[@]}"
echo "${arr[@]}"
IFS=:
j=(a b c)
echo "${j[*]}"
echo "${j[@]}"
oldifs="$IFS"
IFS='|'
echo "${j[*]}"
IFS="$oldifs"
arr2=($(printf 'p q\nr\n'))
echo "${#arr2[@]}"
echo "${arr2[*]}"
