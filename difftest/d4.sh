# arrays in function + local + global write back
garr=(g1 g2)
setg() { garr[0]=GG; }
setg
echo "${garr[@]}"
f() {
  local larr=(l1 l2)
  echo "in:${larr[@]}"
  larr[0]=LZ
  garr[1]=g2mod
  echo "in2:${larr[*]}"
}
f
echo "out:${garr[@]}"
echo "after-local:$larr" 2>&1 || true
echo "${garr[*]}"
