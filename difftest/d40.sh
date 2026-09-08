#!/bin/bash
# Herestring with array and command substitution
arr=(h1 h2 h3)
result=$(cat <<< "${arr[@]}")
echo "result:[$result]"
