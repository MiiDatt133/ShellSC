echo "sub:$(eval 'echo inner')"
r=$(eval 'echo cmdsub')
echo "r=$r"
eval 'out=`echo backquote`'
echo "out=$out"
eval "nested=\$(eval 'echo deep')"
echo "nested=$nested"
