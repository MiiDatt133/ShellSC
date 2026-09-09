# f11: bg function subshell semantics — parent vars unchanged
pvar=before
mutfn() { pvar=after; echo "child sees:$pvar"; }
mutfn &
wait
echo "parent sees:$pvar"
g=orig
sub() { g=changed; }
sub &
wait
echo "g=$g"
# bg function with pipeline inside
pipefn() { echo "x" | tr 'a-z' 'A-Z'; }
pipefn &
wait
