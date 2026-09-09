# f47: isolated — bg fn with both > and 2> redirect (f16 tail repro)
o() { echo "O"; echo "E" >&2; }
rm -f f47o.txt f47e.txt
o > f47o.txt 2> f47e.txt &
wait
echo "o:[$(cat f47o.txt)]e:[$(cat f47e.txt)]"
rm -f f47o.txt f47e.txt
