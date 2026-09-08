#!/bin/bash
# Herestring with arrays - quoted vs unquoted
arr=(hello world)
cat <<< "${arr[@]}"
echo "---"
cat <<< "${arr[*]}"
echo "---"
cat <<< "${arr[0]}"
