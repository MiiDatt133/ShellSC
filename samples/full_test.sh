echo "=== shellsc full syntax test ==="

echo "--- assignment ---"
X=hello
Y=world
echo "$X $Y"

echo "--- export / unset ---"
export A=foo B=bar
echo "$A $B"
unset A
echo "A after unset: $A"

echo "--- if / elif / else ---"
V=2
if test "$V" = "1"; then
    echo "one"
elif test "$V" = "2"; then
    echo "two"
else
    echo "other"
fi

echo "--- while ---"
N=0
while test "$N" -lt 3; do
    echo "N=$N"
    N=1
    if test "$N" -lt 1; then N=0; fi
    break
done
echo "while done"

echo "--- for ---"
for item in alpha beta gamma; do
    echo "item: $item"
done

echo "--- for break / continue ---"
for i in 1 2 3 4 5; do
    if test "$i" = "2"; then continue; fi
    if test "$i" = "4"; then break; fi
    echo "i=$i"
done

echo "--- case ---"
for word in hello world other; do
    case $word in
        hello) echo "matched hello" ;;
        world) echo "matched world" ;;
        *)     echo "no match: $word" ;;
    esac
done

echo "--- pipeline ---"
echo "hello world" | grep -o "hello"

echo "--- boolean chain ---"
true && echo "and ok"
false || echo "or ok"
false && echo "unreachable"

echo "--- background ---"
sleep 0 &

echo "--- redirect ---"
echo "line one" > shellsc_test.txt
echo "line two" >> shellsc_test.txt
read LINE < shellsc_test.txt
echo "read: $LINE"

echo "--- printf ---"
printf "%s + %s = %s\n" "foo" "bar" "foobar"

echo "--- test ---"
test -n "$X" && echo "X is set"
test -z "$A" && echo "A is empty"

echo "--- sleep ---"
sleep 0.5

echo "=== all done ==="