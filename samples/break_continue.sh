for i in 1 2 3 4 5; do
    if test "$i" = "3"; then
        echo "skipping 3"
        continue
    fi
    if test "$i" = "5"; then
        echo "breaking at 5"
        break
    fi
    echo "i = $i"
done
echo "after for"