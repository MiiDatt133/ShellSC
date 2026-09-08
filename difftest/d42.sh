#!/bin/bash
# Complex: function redirect + local array + array-from-array + herestring
worker() {
  local src=(w1 w2 w3)
  local copy=("${src[@]}")
  copy+=(w4)
  local line
  line=$(cat <<< "${copy[@]}")
  echo "worker:$line"
  echo "src_count:${#src[@]}"
  echo "copy_count:${#copy[@]}"
} 2>/dev/null

worker
worker
