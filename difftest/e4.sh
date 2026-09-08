myfn() {
  eval 'inner=set_in_fn'
  echo "in-fn: $inner"
  eval 'localv=loc'
}
myfn
echo "after: $inner"
echo "after-local: $localv"
