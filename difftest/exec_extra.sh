# exec extra cases
exec 4> fd4.txt
echo "to fd4" >&4
cat fd4.txt
echo before
exec > redirect_all.txt
echo "captured in file"
exec 1>&2
echo "to stderr after exec redirect" >&2
cat redirect_all.txt
