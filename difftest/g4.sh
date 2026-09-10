fib() {
  if [ "$1" -le 1 ]; then echo "$1"; return; fi
  a=$(fib $(($1-1)))
  b=$(fib $(($1-2)))
  echo $(($a+$b))
}
fib 8
