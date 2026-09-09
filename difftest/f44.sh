# f44: param default forms with positional
set -- A B
echo "${1:-D}"
echo "${3:-D}"
echo "${1-X}"
echo "${3:+SET}"
echo "${1:+YES}"
echo "${#1}"
echo "${1:?msg}" 2>/dev/null || echo "err-branch"
echo "${1}tail"
