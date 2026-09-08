#!/bin/bash
# Trap + array interaction
arr=(traptest)
trap 'echo "caught:${arr[@]}"' EXIT
arr+=(more)
echo "main:${arr[@]}"
