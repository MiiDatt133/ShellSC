# f33: isolated — here-string replacing stdin cursor after heredoc feed
while read -r line; do echo "L:$line"; done <<< "A B C"
read x <<< "tail"
echo "x=$x"
