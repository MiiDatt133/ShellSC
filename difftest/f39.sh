# f39: isolated — two bg function calls on separate lines
p() { echo "ok"; }
p &
p &
wait
echo "done"
