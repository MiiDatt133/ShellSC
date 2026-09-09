# f8: heredoc positional params — function + locals
fn() {
  echo "in-fn:1=$1:2=$2:#=$#"
  local lv=L
  cat <<EOF
fn-heredoc:1=$1:2=$2:$#:$*:$@:local=$lv
EOF
  cat <<'EOF'
quoted:1=$1:no-expand:$#
EOF
}
fn A B
set -- X Y Z
cat <<EOF
outer:$1:$2:$3:$#:$?
EOF
echo "after:$1"
