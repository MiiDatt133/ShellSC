# f21: isolated — $'\cZ' control chars (f4 build aborted earlier)
printf '%s' $'\ca' | od -An -c
printf '%s' $'\cA' | od -An -c
printf '%s' $'\q\z' | od -An -c
printf '%s' $'' | od -An -c
