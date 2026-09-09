# f32: isolated — procsub stdin <( ) parse (f18) — check sc parse error
read b < <(echo second)
echo "b=$b"
