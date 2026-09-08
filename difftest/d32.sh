#!/bin/bash
# Quoted vs unquoted ${arr[@]} in various contexts
arr=("a b" "c d")
echo "quoted:"
for x in "${arr[@]}"; do echo "[$x]"; done
echo "unquoted:"
for x in ${arr[@]}; do echo "[$x]"; done
