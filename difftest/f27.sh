# f27: isolated — $'\c' unsupported control syntax + $'a\'b' (f19 tail / f6)
printf '%s' $'\ca' | od -An -c
printf '%s' $'\cA' | od -An -c
printf '%s' $'\c[' | od -An -c
