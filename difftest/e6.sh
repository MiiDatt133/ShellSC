eval 'echo hi > e6_out.txt'
cat e6_out.txt
eval 'echo more >> e6_out.txt'
cat e6_out.txt
eval 'wc -c < e6_out.txt'
echo "rc=$?"
rm -f e6_out.txt
