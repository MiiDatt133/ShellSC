# function features
counter() {
  c=$((c+1))
  return $((c * 10))
}
c=0
counter
echo "status=$?"
counter
echo "status=$?"
echo "direct=$?"
early() {
  [ "$1" = "stop" ] && return 5
  echo "continued"
}
early "go"
echo "e1=$?"
early "stop"
echo "e2=$?"
