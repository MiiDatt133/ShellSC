# array element in [[ ]] tests, case with array
a=(alpha beta "gamma delta")
[[ "${a[2]}" == "gamma delta" ]] && echo "T1"
[[ ${a[2]} == "gamma "* ]] && echo "T2"
[[ "${#a[@]}" -eq 3 ]] && echo "T3"
[[ "${a[1]}" < "${a[2]}" ]] && echo "T4"
case "${a[0]}" in
  alp*) echo "C1" ;;
  *) echo "C2" ;;
esac
for e in "${a[@]}"; do
  case "$e" in
    g*d*) echo "M:$e" ;;
    *) echo "m:$e" ;;
  esac
done
[[ -n "${a[5]}" ]] || echo "T5-noval"
