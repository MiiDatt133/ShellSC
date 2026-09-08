#!/bin/bash
# Function redirect + nested call + array passing
log() {
  echo "LOG:$@"
} 2>/dev/null

process() {
  local data=(item1 item2 item3)
  log "${data[@]}"
}

process
process
