# f9: heredoc params with set --, $0, here-strings
set -- p1 p2
cat <<EOF
ps:$1,$2,$#,first=$1
star=$*
at=$@
EOF
printf '%s\n' "$0" > /dev/null
cat <<EOF
zero=$0
EOF
read -r a b <<< "$1 rest"
echo "hs:$a:$b"
fn2() { read -r x y <<< "$1 $2"; echo "fn2:$x:$y"; }
fn2 Q R
