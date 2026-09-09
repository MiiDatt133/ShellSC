# f31: isolated — $'...' delimiter heredoc at EOF + $'\c' mid-word, mixed concat
# default IFS display via printf direct
printf '%s\n' "$IFS" | od -An -c | head -1
unset IFS
printf '%s\n' "${IFS:-UNSET}" | head -1
