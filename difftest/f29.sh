# f29: isolated — $'a\'b' escaped single-quote inside ANSI-C
printf '%s' $'a\'b' | od -An -c
printf '%s' $'it\'s' | od -An -c
echo $'quo'"te" | od -An -c
