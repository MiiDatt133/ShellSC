# and-or lists, negation, grouping
false && echo "no" || echo "yes"
! true
echo "neg=$?"
! false
echo "neg2=$?"
if ! false; then echo "notted"; fi
{ true; } && { echo "group-ok"; }
(echo "paren"; true) && echo "paren-ok"
a=1; b=2
[ "$a" = "1" ] && [ "$b" = "2" ] && echo "both-eq"
