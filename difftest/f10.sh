# f10: bg function calls
bgfn() { echo "bgfn:$1:$2"; }
bgfn A B &
wait
echo "after-wait:$?"
bgfn2() { echo "w" > bgout2.txt; }
rm -f bgout2.txt
bgfn2 &
wait
echo "file:$(cat bgout2.txt)"
rm -f bgout2.txt
bgfn3() { echo "redirected"; }
bgfn3 > bgout3.txt &
wait
echo "redir:$(cat bgout3.txt)"
rm -f bgout3.txt
sleep 0.05 &
wait
echo "bgext:$?"
