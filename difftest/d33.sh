#!/bin/bash
# Nested function calls with arrays and redirects
outer() {
  local oarr=(o1 o2)
  inner "${oarr[@]}"
} 2>/dev/null

inner() {
  local iarr=("$@")
  echo "inner got:${iarr[@]}"
  echo "inner count:${#iarr[@]}"
}

outer
