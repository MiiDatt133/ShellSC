greet() {
    echo "hello, $1"
}

add() {
    echo "$1 + $2"
}

function repeat {
    local_word=$1
    count=$2
    i=0
    while test "$i" -lt "$count"; do
        echo "$local_word"
        i=1
        break
    done
}

greet world
greet shellsc
add 3 5
repeat hi 3

result=0
check() {
    if test "$1" = "ok"; then
        echo "check passed"
    else
        echo "check failed"
    fi
}
check ok
check fail