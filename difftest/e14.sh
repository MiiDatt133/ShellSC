eval 'case abc in a*) echo A-Star;; esac'
eval 'case xyz in
  a*) echo A;;
  x*) echo X;;
  *) echo Star;;
esac'
v=banana
eval "case \$v in
  apple) echo APPLE;;
  ban*) echo BAN;;
  *) echo OTHER;;
esac"
eval 'for w in apple fig; do case $w in ap*) echo AP-$w;; *) echo O-$w;; esac; done'
echo endcase
