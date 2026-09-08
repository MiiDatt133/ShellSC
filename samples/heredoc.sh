echo "[1] Basic heredoc"
cat <<EOF
Hello World
EOF

echo "[2] Pipeline heredoc"
cat <<EOF | wc -l
one
two
three
EOF
