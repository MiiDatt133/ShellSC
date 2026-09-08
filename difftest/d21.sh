# redirects with arrays: 2>&1, <<<
a=(h1 h2)
grep -o "h[12]" <<< "${a[@]}" || true
x=$(cat <<< "${a[0]}-${a[1]}")
echo "hs:[$x]"
printf '%s\n' "err msg" >&2
out=$(printf '%s\n' "both" 2>&1)
echo "cap:[$out]"
echo end
