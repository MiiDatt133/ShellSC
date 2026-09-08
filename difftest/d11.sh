# param expansion corners
f=foo.tar.gz
echo "${f#*.}"
echo "${f##*.}"
echo "${f%.*}"
echo "${f%%.*}"
p="*literal*"
echo "${p#*}"
v=abcdefgh
echo "${v:2}"
echo "${v:2:3}"
s=" spaced "
echo "[$s]"
echo "[$(echo $s)]"
echo "[$(echo "$s")]"
echo "${v:=shouldnotset}"
echo "${v:+nonempty}"
u=
echo "${u:=setnow}"
echo "$u"
echo "pre${#v}post"
