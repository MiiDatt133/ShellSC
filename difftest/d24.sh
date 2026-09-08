#!/bin/bash
# Function redirect suppressing stderr, caller generates stderr
myfunc() {
  echo "stdout"
  echo "stderr" >&2
} 2>/dev/null

myfunc
echo "---"
myfunc 2>&1
