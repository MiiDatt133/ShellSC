for w in hello.c foo.txt Makefile script.sh; do
  case "$w" in
    *.c) echo "$w: C source" ;;
    *.txt) echo "$w: text" ;;
    [Mm]akefile) echo "$w: make" ;;
    *) echo "$w: other" ;;
  esac
done
