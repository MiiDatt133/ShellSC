use clap::{Arg, ArgAction, Command};

pub fn build_cli() -> Command {
    Command::new("shellsc")
        .version("0.1.0")
        .about("Shell script compiler → ELF .sc executable")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("gen")
                .about("Compile .sh → .sbc assembly text")
                .arg(
                    Arg::new("input")
                        .value_name("input.sh")
                        .required(true)
                        .help("Shell script source file"),
                )
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .value_name("FILE")
                        .help("Output path (default: <input>.sbc)"),
                ),
        )
        .subcommand(
            Command::new("pack")
                .about("Assemble .sbc → .bc → .sc ELF executable")
                .arg(
                    Arg::new("input")
                        .value_name("input.sbc")
                        .required(true)
                        .help("SBC assembly text file"),
                )
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .value_name("FILE")
                        .help("Output path (default: <input>.sc)"),
                )
                .arg(
                    Arg::new("protect")
                        .long("protect")
                        .action(ArgAction::SetTrue)
                        .help("Protect the embedded bytecode: XOR encryption (random per-build key), opcode shuffling, anti-debug, CRC32 integrity check"),
                )
                .arg(
                    Arg::new("smc")
                        .long("smc")
                        .action(ArgAction::SetTrue)
                        .requires("protect")
                        .help("Also enable self-modifying bytecode: the program stays encrypted in memory and is decrypted one instruction at a time (implies --protect)"),
                )
                .arg(
                    Arg::new("self-debug")
                        .long("self-debug")
                        .action(ArgAction::SetTrue)
                        .requires("protect")
                        .help("Fork a child that ptrace-attaches the parent, blocking external debuggers"),
                )
                .arg(
                    Arg::new("all")
                        .long("all")
                        .action(ArgAction::SetTrue)
                        .help("Enable all protections: --protect --smc --self-debug"),
                ),
        )
        .subcommand(
            Command::new("build")
                .about("Full pipeline: .sh → .sbc → .bc → .sc ELF")
                .arg(
                    Arg::new("input")
                        .value_name("input.sh")
                        .required(true)
                        .help("Shell script source file"),
                )
                .arg(
                    Arg::new("output")
                        .short('o')
                        .long("output")
                        .value_name("FILE")
                        .help("Output .sc path (default: <input>.sc)"),
                )
                .arg(
                    Arg::new("emit_sbc")
                        .long("emit-sbc")
                        .action(ArgAction::SetTrue)
                        .help("Also write <input>.sbc"),
                )
                .arg(
                    Arg::new("emit_bc")
                        .long("emit-bc")
                        .action(ArgAction::SetTrue)
                        .help("Also write <input>.bc"),
                )
                .arg(
                    Arg::new("emit_all")
                        .long("emit-all")
                        .action(ArgAction::SetTrue)
                        .help("Also write both <input>.sbc and <input>.bc"),
                )
                .arg(
                    Arg::new("protect")
                        .long("protect")
                        .action(ArgAction::SetTrue)
                        .help("Protect the embedded bytecode: XOR encryption (random per-build key), opcode shuffling, anti-debug, CRC32 integrity check"),
                )
                .arg(
                    Arg::new("smc")
                        .long("smc")
                        .action(ArgAction::SetTrue)
                        .requires("protect")
                        .help("Also enable self-modifying bytecode: the program stays encrypted in memory and is decrypted one instruction at a time (implies --protect)"),
                )
                .arg(
                    Arg::new("self-debug")
                        .long("self-debug")
                        .action(ArgAction::SetTrue)
                        .requires("protect")
                        .help("Fork a child that ptrace-attaches the parent, blocking external debuggers"),
                )
                .arg(
                    Arg::new("all")
                        .long("all")
                        .action(ArgAction::SetTrue)
                        .help("Enable all protections: --protect --smc --self-debug"),
                ),
        )
        .subcommand(
            Command::new("dump-ast")
                .about("Print AST for a .sh file")
                .arg(Arg::new("input").value_name("input.sh").required(true)),
        )
        .subcommand(
            Command::new("dump-ir")
                .about("Print IR ops for a .sh file")
                .arg(Arg::new("input").value_name("input.sh").required(true)),
        )
        .subcommand(
            Command::new("disasm")
                .about("Disassemble a .sbc file to stdout")
                .arg(Arg::new("input").value_name("input.sbc").required(true)),
        )
}
