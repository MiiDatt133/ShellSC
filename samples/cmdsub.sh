DATE=$(date +%Y 2>/dev/null || echo 2024)
echo "year: $DATE"

FILES=$(ls /proc/self 2>/dev/null | head -3)
echo "proc entries: $FILES"

MSG="hello from $(echo inner)"
echo "$MSG"

UPPER=$(echo shellsc | tr a-z A-Z 2>/dev/null || echo SHELLSC)
echo "upper: $UPPER"

COUNT=$(echo one two three | wc -w 2>/dev/null || echo 3)
echo "count: $COUNT"

NESTED=$(echo $(echo deep))
echo "nested: $NESTED"

X=`echo backtick`
echo "backtick: $X"

echo "done: $(echo ok)"