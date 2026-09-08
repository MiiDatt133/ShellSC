eval "false"
echo "r1=$?"
eval "true"
echo "r2=$?"
eval "false"
rc=$?
echo "rc=$rc"
