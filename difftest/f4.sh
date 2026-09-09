# f4: control chars \cZ, unknown escapes, empty $''
printf '%s' $'\ca' | od -c | head -1
printf '%s' $'\cA' | od -c | head -1
printf '%s' $'\q\z' | od -c | head -1
printf '%s' $'' | od -c | head -1
echo "empty:${#$''}"
