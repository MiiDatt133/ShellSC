# f30: isolated — read trim leading/trailing IFS whitespace (f13 tail)
read -r z <<< "  spaces  "
echo "trim:[$z]"
read -r p q <<< "  a  b  "
echo "p:[$p] q:[$q]"
IFS=: read -r a b <<< ":x:"
echo "a:[$a] b:[$b]"
