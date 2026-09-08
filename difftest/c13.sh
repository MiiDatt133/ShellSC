# exec N> file
exec 5> fd5.txt
echo "hello to fd" >&5
cat fd5.txt
