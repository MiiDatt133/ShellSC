# arrays + traps, wait-like, exit code, subshell isolation
arr=(a b)
trap 'echo "trap:${arr[@]}"' EXIT
(
  arr[0]=mod
  echo "sub:${arr[@]}"
)
echo "after:${arr[@]}"
f() {
  local arr=(loc1 loc2)
  echo "f:${arr[@]}"
  ( echo "fsub:${arr[@]}" )
}
f
echo "g:${arr[@]}"
echo done
