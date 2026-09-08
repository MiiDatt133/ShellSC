use_local() {
  eval 'local lv=inside'
  echo "in: $lv"
}
use_local
echo "out: ${lv-unset}"
both() {
  local pre=pre
  eval 'pre=changed'
  echo "both: $pre"
}
both
