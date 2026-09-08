#!/bin/bash
# Local array vs global array interaction
garr=(global1 global2)
myfunc() {
  local larr=(local1 local2)
  echo "local:${larr[@]}"
  echo "global:${garr[@]}"
  garr=(modified)
}
myfunc
echo "after:${garr[@]}"
