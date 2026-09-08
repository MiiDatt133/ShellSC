eval 'true'
echo "t=$?"
eval 'false'
echo "f=$?"
if eval 'false'; then echo THEN; else echo ELSE; fi
eval 'exit 0' ; echo unreachable
echo after
