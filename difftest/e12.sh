eval 'q() { echo "Q $1"; }; q one; q two'
eval 'r() { echo R; }; for i in 1 2; do r; done'
s() { echo S-before; }
eval 's() { echo S-new; }; s'
s
