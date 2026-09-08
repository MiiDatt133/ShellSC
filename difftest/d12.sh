# arrays + glob, arrays in case, quotes concat
set -- *.sh
echo "match:$#"
arr=(*.sh)
echo "${arr[@]}"
mkdir -p dt_tmp
touch dt_tmp/a.txt dt_tmp/b.txt
g=(dt_tmp/*.txt)
echo "${#g[@]}"
for f in "${g[@]}"; do echo "F:$f"; done
case "${arr[0]}" in
  *.sh) echo "case:sh" ;;
  *) echo "case:other" ;;
esac
echo "a${g[0]}b" "c${g[1]}"d
rm -rf dt_tmp
