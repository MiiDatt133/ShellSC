fn() {
  local arr=(a b c)
  echo "${arr[1]}"
}
fn
echo "${arr[1]:-gone}"
