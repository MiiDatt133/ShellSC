#!/bin/bash
# Trap body compiled in child VM accessing arrays
arr=(before)
trap 'arr+=(trapped); echo "trap sees:${arr[@]}"' EXIT
arr=(after)
echo "main:${arr[@]}"
