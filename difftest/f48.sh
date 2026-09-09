# f48: isolated — read -a backslash handling without -r (f12 repro)
printf 'a\\tb\n' | { read -a arr; echo "n:${#arr[@]}:0:[$arr]:0b:[${arr[0]}]:1b:[${arr[1]}]"; }
printf 'a\\tb\n' | { read -r -a arr; echo "raw:0:[${arr[0]}]:1:[${arr[1]}]"; }
printf 'x\\ y\n' | { read -a arr; echo "esc-space:0:[${arr[0]}]:1:[${arr[1]}]"; }
