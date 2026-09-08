# functions returning via global array; array pass through cmdsub; printf %s array
mk() {
  res=(r1 r2 r3)
}
mk
echo "${res[@]}"
consumer() {
  local in=("$@")
  echo "got:${#in[@]}"
  echo "${in[@]}"
}
consumer "${res[@]}"
out=$(mk; echo extra)
echo "out:$out"
printf '%s\n' "${res[@]}"
printf '[%s]' "${res[@]}"; echo
printf '%s|%s\n' "${res[0]}" "${res[1]}"
