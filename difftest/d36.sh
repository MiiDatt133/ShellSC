#!/bin/bash
# Array expansion in case pattern
arr=(foo bar)
val="bar"
case "$val" in
  "${arr[0]}") echo "match first" ;;
  "${arr[1]}") echo "match second" ;;
  *) echo "no match" ;;
esac
