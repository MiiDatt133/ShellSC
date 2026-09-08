#!/bin/bash
# Sparse array indices
arr=()
arr[0]=a
arr[5]=f
arr[2]=c
echo "${arr[@]}"
echo "${#arr[@]}"
echo "${arr[5]}"
unset arr[5]
echo "${#arr[@]}"
echo "${arr[@]}"
