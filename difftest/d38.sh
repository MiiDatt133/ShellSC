#!/bin/bash
# Array index arithmetic
arr=(zero one two three four)
i=2
echo "${arr[$i]}"
echo "${arr[i+1]}"
echo "${arr[$((i+2))]}"
