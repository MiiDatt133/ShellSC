eval 'i=0; while [ $i -lt 3 ]; do echo "L$i"; i=$((i+1)); done'
eval 'for f in a b c; do echo "F:$f"; done'
eval 'if [ 1 -lt 2 ]; then echo YES; else echo NO; fi'
eval 'for n in 1 2 3 4; do if [ $n -gt 2 ]; then echo big $n; fi; done'
echo done
