# env prefix + array; export array (bash ignores); array name reuse as scalar
arr=(a b)
echo "${arr[@]}"
export arr
echo "${arr[@]}"
arr=scalar
echo "$arr"
echo "${arr[@]}"
echo "${#arr[@]}"
newvar=ok
echo "$newvar"
arr=(x y)
echo "${arr[@]}"
