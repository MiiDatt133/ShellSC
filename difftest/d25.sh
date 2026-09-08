#!/bin/bash
# Empty array edge cases
arr=()
echo "count:${#arr[@]}"
echo "all:[${arr[@]}]"
echo "star:[${arr[*]}]"
for x in "${arr[@]}"; do echo "iter:$x"; done
echo "done"
