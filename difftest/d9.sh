# for over "$@", "$*", "$#" with set --
set -- one "two three" four
echo "n=$#"
for a in "$@"; do echo "at:[$a]"; done
for a in "$*"; do echo "star:[$a]"; done
for a in $*; do echo "ustar:[$a]"; done
shift
echo "n=$#"
echo "first:$1"
set --
echo "n=$#"
for a in "$@"; do echo "never:$a"; done
echo "end"
