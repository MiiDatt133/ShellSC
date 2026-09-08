ARG=hello

case $ARG in
    hello)
        echo "got hello"
        ;;
    bye)
        echo "got bye"
        ;;
    *)
        echo "unknown: $ARG"
        ;;
esac

for x in foo bar baz; do
    case $x in
        foo|bar)
            echo "$x is foo or bar"
            ;;
        *)
            echo "$x is other"
            ;;
    esac
done