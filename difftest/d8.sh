# heredoc with arrays and expansions
arr=(alpha beta)
cat <<EOF
1:${arr[0]} 2:${arr[1]}
all:${arr[@]}
count:${#arr[@]}
EOF
cat <<-END
	indented:${arr[0]}
END
name=world
cat <<EOF
mixed:${name}/${arr[1]}
EOF
cat <<X
Xafter:${arr[0]}
X
echo fin
