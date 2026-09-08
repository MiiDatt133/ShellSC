eval 'v=hello'
echo "v=$v"
eval 'w=one' 'w2=two'
echo "w=$w w2=$w2"
eval "k=$(echo nested)"
echo "k=$k"
