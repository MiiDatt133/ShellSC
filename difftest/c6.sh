# subshell & env isolation
x=outer
(echo "in=$x"; x=inner; echo "set=$x")
echo "out=$x"
x=out1 y=out2 sh -c 'echo "env=$x/$y"' 2>/dev/null || echo "sh not ok"
