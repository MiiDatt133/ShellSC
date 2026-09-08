# quoted vs unquoted arrays in for-loop, glob interplay
set -f
files=(one two)
for x in ${files[@]}; do echo "u:$x"; done
for x in "${files[@]}"; do echo "q:$x"; done
for x in "${files[*]}"; do echo "star:[$x]"; done
set +f
sp=("a 1" "b 2")
for x in ${sp[@]}; do echo "U:$x"; done
for x in "${sp[@]}"; do echo "Q:$x"; done
for x in "${sp[*]}"; do echo "S:$x"; done
echo "IFS-join: ${sp[*]}"
