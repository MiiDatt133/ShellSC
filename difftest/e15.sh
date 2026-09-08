pre() { echo "before-exit"; }
pre
eval 'exit 3'
echo "never reached"
