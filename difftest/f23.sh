# f23: isolated — case pattern with $'\t'
v=$'x\ty'
case $v in
  *$'\t'*) echo "tab-in-case" ;;
  *) echo "no-match" ;;
esac
case $'\t' in
  $'\t') echo "exact-tab" ;;
esac
