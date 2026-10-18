# Concurrent stdout order of `echo A &` etc. is undefined in bash itself —
# capture each job to a file so the comparison stays deterministic.
d=$(mktemp -d)
echo A >"$d/a" &
echo B >"$d/b" &
echo C >"$d/c" &
wait
cat "$d/a" "$d/b" "$d/c"
echo done
rm -rf "$d"
