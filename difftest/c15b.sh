# exec N< missing-file: open fails, fd stays unusable.
# bash: reports the open failure, then `head -1 <&6` fails with
# "6: Bad file descriptor" — script continues, final exit is 1 (head's).
# The .sc error prefix differs from bash's "$0: line N:" prefix (the
# compiled binary does not track source lines); compare exit code and
# the Bad-file-descriptor message presence, not exact bytes.
exec 6< fdin_missing.txt
head -1 <&6
