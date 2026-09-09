# f35: isolated — case pattern ANSI-C 2 variants (f23 minimal)
v=$'x\ty'
case $v in
  *$'\t'*) echo M1 ;;
  *) echo N1 ;;
esac
case ab in
  a$'b') echo M2 ;;
esac
case $'\ta' in
  $'\t'a) echo M3 ;;
esac
