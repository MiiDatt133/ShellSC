# f46: bg fn stdout goes to parent stdout? order with echo markers
m() { echo "bg-line"; }
m &
wait
echo "parent-line"
m > /dev/null &
wait
echo "end"
