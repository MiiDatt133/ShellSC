# pipelines + cmdsub interplay
echo "count: $(echo a b c | wc -w)"
r=$(printf "x\ny\n" | head -1)
echo "r=$r"
echo "nested: $(echo $(echo deep))"
echo "quoted cmdsub: \"$(echo 'a b')\""
