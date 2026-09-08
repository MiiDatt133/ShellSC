# exec N< file: open fd, read from it, then verify missing-file handling.
# Self-contained: creates fdin.txt itself so bash and .sc see the same world.
# bash: line1 then exit 0. The missing-file case is covered by c15b.
printf 'line1\nline2\n' > fdin.txt
exec 6< fdin.txt
head -1 <&6
rm -f fdin.txt
