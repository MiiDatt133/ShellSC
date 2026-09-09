# f7: ANSI-C in heredoc delimiters, arrays, printf formats
read -r line <<$'END\tX'
hello
$'END\tX'
echo "line:$line"
arr=($'a\t1' $'b\t2')
echo "n:${#arr[@]}"
echo "0:${arr[0]}|1:${arr[1]}"
printf $'%s\t%s\n' A B
printf $'%s\n' x
