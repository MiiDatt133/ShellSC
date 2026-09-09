# f18: here-string cursor vs heredoc vs pipe stdin chains
read a <<< "first"
read b < <(echo second) 2>/dev/null || echo "procsub-skip"
read c <<< "third"
echo "$a|$c"
read d e <<< "x y"
read f <<< "z"
echo "d=$d e=$e f=$f"
cat <<< "hs" | { read r1; echo "pipeds:$r1"; }
read g <<< "after"
echo "g=$g"
while read -r w; do echo "W:$w"; done <<< "A B"
echo "post:$?"
