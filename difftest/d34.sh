#!/bin/bash
# Array append += with single element and multiple
arr=(start)
arr+=(mid)
arr+=(end1 end2)
echo "${arr[@]}"
echo "${#arr[@]}"
