# f22: isolated — unknown escapes \q \z and empty $''
printf '%s' $'\q' | od -An -c
printf '%s' $'\z' | od -An -c
printf '%s' $'' | od -An -c
