# f24: isolated — heredoc with ANSI-C quoted delimiter
cat <<$'END'
literal $1 $(x) $HOME
END
echo "==="
cat <<$'EN\D'
body
EN\D
echo "second-ok"
