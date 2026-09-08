#!/bin/bash
# Local array shadowing global
arr=(global)
myfunc() {
  local arr=(local1 local2)
  echo "inside:${arr[@]}"
  arr+=(local3)
  echo "inside after:${arr[@]}"
}
myfunc
echo "outside:${arr[@]}"
