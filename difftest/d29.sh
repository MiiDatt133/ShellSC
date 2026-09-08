#!/bin/bash
# IndexDefault on arrays
arr=(one two three)
echo "${arr[0]:-fallback}"
echo "${arr[5]:-fallback}"
echo "${arr[1]:-unused}"
unset arr[1]
echo "${arr[1]:-was_unset}"
