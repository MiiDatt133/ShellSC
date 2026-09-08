# arr[i] element + default/slice ops NOT parseable — record as divergence.
# Here: only ops that parse.
arr=(a b c)
unset nu
echo "${nu:-plaindefault}"
echo "${#nu[@]}"
echo "${arr[@]}"
