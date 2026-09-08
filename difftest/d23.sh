#!/bin/bash
# Function redirect + array inside function
myfunc() {
  arr=(x y z)
  echo "${arr[@]}"
} 2>/dev/null

myfunc
myfunc
