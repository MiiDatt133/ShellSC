#!/bin/bash
# Array-from-array with modifications
src=(alpha beta gamma)
dst=("${src[@]}")
echo "dst:${dst[@]}"
echo "count:${#dst[@]}"
dst+=(delta)
echo "after:${dst[@]}"
echo "srccount:${#src[@]}"
