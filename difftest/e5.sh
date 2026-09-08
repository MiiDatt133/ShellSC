eval 'a=first'
echo "a=$a"
eval 'b=second'
echo "b=$b"
eval 'echo "sees a=$a b=$b"'
eval 'c=third'
eval 'd=fourth'
eval 'echo "final a=$a b=$b c=$c d=$d"'
