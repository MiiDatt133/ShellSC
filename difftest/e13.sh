eval 'm=stage1'
eval "echo ref=\$m"
eval 'n=stage2'
eval "echo m=\$m n=\$n"
eval 'eval "echo deep-eval"; echo tail'
echo end
