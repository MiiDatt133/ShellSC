# f6: unterminated $', nested $'\''', quotes in args
echo start
printf '%s\n' $'it''s'
v=$'quo'"te"
echo "$v"
x=$'a\'b'
echo "$x" | od -c | head -1
echo done
