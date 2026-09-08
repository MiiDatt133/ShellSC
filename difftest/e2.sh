eval 'g() { echo G; }'
g
eval 'h() { echo H $1; }'
h arg1
f() { echo F; }
eval 'g() { echo G2; }'
g
