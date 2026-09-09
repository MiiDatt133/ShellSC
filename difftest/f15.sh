# f15: ANSI-C quoted heredoc delimiters + expansions
cat <<$'EOF'
$notexpanded
EOF
cat <<$'END'
data $1
END
echo "---"
cat <<END
expand:$(echo inline)
END
printf '%s\n' "$'x'" 2>&1 | head -1
echo $'END' | cat
