use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::ArgMatches;
use shell_bc::{SbcArg, SbcParser};
use shell_ir::Lowerer;
use shell_lex::Lexer;
use shell_parse::Parser;

use crate::pipeline::{
    full_pipeline, make_executable, map_shell_err, output_path, read_source, sbc_to_bytecode,
    sh_to_sbc, write_bytes, write_text,
};

pub fn dispatch(matches: ArgMatches) -> Result<()> {
    match matches.subcommand() {
        Some(("gen", m)) => cmd_gen(m),
        Some(("pack", m)) => cmd_pack(m),
        Some(("build", m)) => cmd_build(m),
        Some(("dump-ast", m)) => cmd_dump_ast(m),
        Some(("dump-ir", m)) => cmd_dump_ir(m),
        Some(("disasm", m)) => cmd_disasm(m),
        _ => bail!("unknown subcommand"),
    }
}

fn cmd_gen(m: &ArgMatches) -> Result<()> {
    let input = PathBuf::from(m.get_one::<String>("input").unwrap());
    let output = output_path(
        &input,
        "sbc",
        m.get_one::<String>("output").map(|s| s.as_str()),
    );

    let src = read_source(&input)?;
    let sbc = sh_to_sbc(&src)?;

    write_text(&output, &sbc)?;
    eprintln!("gen: {} → {}", input.display(), output.display());
    Ok(())
}

fn cmd_pack(m: &ArgMatches) -> Result<()> {
    let input = PathBuf::from(m.get_one::<String>("input").unwrap());
    let output = output_path(
        &input,
        "sc",
        m.get_one::<String>("output").map(|s| s.as_str()),
    );

    let sbc_text = std::fs::read_to_string(&input)
        .with_context(|| format!("reading '{}'", input.display()))?;

    let all = m.get_flag("all");
    let protect = all || m.get_flag("protect");
    let smc = all || m.get_flag("smc");
    let selfdebug = all || m.get_flag("self-debug");

    let bc = sbc_to_bytecode(&sbc_text)?;
    let bc_bytes = bc.to_bytes();
    let sc_bytes = if protect {
        let mut opts = shell_pack::ProtectOptions::all();
        opts.smc = smc;
        opts.selfdebug = selfdebug;
        shell_pack::Packer::new()
            .pack_protected(&bc_bytes, opts)
            .map_err(map_shell_err)?
    } else {
        shell_pack::Packer::new()
            .pack(&bc_bytes)
            .map_err(map_shell_err)?
    };

    write_bytes(&output, &sc_bytes)?;
    make_executable(&output)?;
    eprintln!("pack: {} → {}", input.display(), output.display());
    Ok(())
}

fn cmd_build(m: &ArgMatches) -> Result<()> {
    let input = PathBuf::from(m.get_one::<String>("input").unwrap());
    let output = output_path(
        &input,
        "sc",
        m.get_one::<String>("output").map(|s| s.as_str()),
    );

    let emit_all = m.get_flag("emit_all");
    let emit_sbc = m.get_flag("emit_sbc") || emit_all;
    let emit_bc = m.get_flag("emit_bc") || emit_all;
    let all = m.get_flag("all");
    let protect = all || m.get_flag("protect");
    let smc = all || m.get_flag("smc");
    let selfdebug = all || m.get_flag("self-debug");

    let src = read_source(&input)?;
    let (sbc_text, bc_bytes, sc_bytes) = full_pipeline(&src, protect, smc, selfdebug)?;

    if emit_sbc {
        let sbc_path = input.with_extension("sbc");
        write_text(&sbc_path, &sbc_text)?;
        eprintln!("emit: {}", sbc_path.display());
    }

    if emit_bc {
        let bc_path = input.with_extension("bc");
        write_bytes(&bc_path, &bc_bytes)?;
        eprintln!("emit: {}", bc_path.display());
    }

    write_bytes(&output, &sc_bytes)?;
    make_executable(&output)?;
    eprintln!("build: {} → {}", input.display(), output.display());
    Ok(())
}

fn cmd_dump_ast(m: &ArgMatches) -> Result<()> {
    let input = PathBuf::from(m.get_one::<String>("input").unwrap());
    let src = read_source(&input)?;

    let tokens = Lexer::new(&src).tokenize().map_err(map_shell_err)?;
    let script = Parser::new(tokens).parse().map_err(map_shell_err)?;

    println!("{:#?}", script);
    Ok(())
}

fn cmd_dump_ir(m: &ArgMatches) -> Result<()> {
    let input = PathBuf::from(m.get_one::<String>("input").unwrap());
    let src = read_source(&input)?;

    let tokens = Lexer::new(&src).tokenize().map_err(map_shell_err)?;
    let script = Parser::new(tokens).parse().map_err(map_shell_err)?;
    let chunk = shell_ir::opt::optimize(Lowerer::new().lower(&script).map_err(map_shell_err)?);

    for (i, op) in chunk.ops.iter().enumerate() {
        println!("{:04}  {:?}", i, op);
    }
    Ok(())
}

fn cmd_disasm(m: &ArgMatches) -> Result<()> {
    let input = PathBuf::from(m.get_one::<String>("input").unwrap());

    let sbc_text = std::fs::read_to_string(&input)
        .with_context(|| format!("reading '{}'", input.display()))?;

    let instrs = SbcParser::new(&sbc_text).parse().map_err(map_shell_err)?;

    for (i, instr) in instrs.iter().enumerate() {
        let args: Vec<String> = instr
            .args
            .iter()
            .map(|a| match a {
                SbcArg::Str(s) => format!("\"{}\"", s.replace('"', "\\\"")),
                SbcArg::Uint(n) => n.to_string(),
            })
            .collect();

        if args.is_empty() {
            println!("{:04}  {}", i, instr.mnemonic);
        } else {
            println!("{:04}  {}  {}", i, instr.mnemonic, args.join(" "));
        }
    }
    Ok(())
}
